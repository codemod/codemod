//! Types from the TypeScript 7 checker (`tsc --api`, the Go port), driven as
//! a side process. Opt-in: `semanticAnalysis: { mode: "workspace", types: "ts7" }`.
//!
//! The wire protocol is TypeScript's sync API (`typescript/unstable/sync`):
//! MessagePack tuples `[type, method, payload]` over the child's stdin and
//! stdout, with JSON payloads. It is unversioned, so the checker must be the
//! pinned TypeScript minor ([`PINNED_VERSION`]); another version is not
//! used. Positions on the wire are UTF-16 offsets. Declarations come back as
//! node handles (`index.kind.path`), an index into the file's binary AST
//! (`getSourceFile`), where each node is a fixed 28-byte record with its
//! `pos` and `end`.
//!
//! With types on, the checker is `CODEMOD_TS_API` (path to the binary, e.g.
//! one baked into a runner image; `off` disables types even when a
//! workflow asks for them), else the workspace's own install of TypeScript 7
//! (`node_modules/@typescript/typescript-<os>-<arch>/lib/tsc`). The variable
//! only says where the checker is: it does not turn types on. Nothing is
//! spawned until the first question, and a checker that fails to start or
//! answer is not tried again: questions then get no answer.
//!
//! Answers describe the step's start, like every semantic answer: the
//! checker reads each file once per step. It reads them through us
//! (`--callbacks readFile,fileExists`): a file the run told the provider
//! about (an earlier step's edit, which a dry run keeps in memory) comes from
//! [`FileTexts`], anything else from the disk.

use language_core::ByteRange;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The TypeScript minor this client speaks. `tsc --version` must start
/// with `Version <PINNED_VERSION>.`.
pub(crate) const PINNED_VERSION: &str = "7.0";

/// Most members an interface or property signature may resolve to. Past
/// that the call is left unresolved (with the signature as its declaration):
/// a member that many objects write is not worth one edge per object.
pub(crate) const MAX_IMPLEMENTATIONS: usize = 16;

const MSG_REQUEST: u8 = 1;
const MSG_CALL_RESPONSE: u8 = 2;
const MSG_CALL_ERROR: u8 = 3;
const MSG_RESPONSE: u8 = 4;
const MSG_ERROR: u8 = 5;
const MSG_CALL: u8 = 6;

const HEADER_OFFSET_NODES: usize = 40;
const NODE_LEN: usize = 28;
const KIND_NODE_LIST: u32 = 0xFFFF_FFFF;

/// TypeScript 7.0 `SyntaxKind`s of nodes that write a member: PropertyDeclaration,
/// MethodDeclaration, GetAccessor, PropertyAssignment, ShorthandPropertyAssignment.
/// The numbers are not a stable contract (see the module docs).
const MEMBER_WRITE_KINDS: [u32; 5] = [173, 175, 178, 303, 304];
const KIND_IDENTIFIER: u32 = 79;
const KIND_PRIVATE_IDENTIFIER: u32 = 80;

/// A declaration the checker pointed at.
#[derive(Debug, Clone)]
pub(crate) struct TsDecl {
    /// The file, as the workspace spells it when known.
    pub path: PathBuf,
    /// The node's range in UTF-16 units, `pos` including leading trivia.
    pub pos16: u32,
    pub end16: u32,
    /// In a library: `node_modules` or a `.d.ts` file.
    pub library: bool,
    /// The checker's node handle, for follow-up questions.
    handle: String,
}

impl TsDecl {
    /// The declaration's syntax kind number (TypeScript 7's `SyntaxKind`).
    #[cfg(test)]
    pub fn kind(&self) -> u32 {
        parse_handle(&self.handle)
            .map(|(_, kind, _)| kind)
            .unwrap_or(0)
    }
}

/// The members a signature resolves to: empty when there are more than
/// [`MAX_IMPLEMENTATIONS`].
#[derive(Debug, Clone, Default)]
pub(crate) struct Related {
    pub decls: Vec<TsDecl>,
}

/// File texts the checker reads instead of the disk.
#[derive(Debug, Default)]
pub(crate) struct FileTexts(parking_lot::RwLock<HashMap<PathBuf, Arc<str>>>);

impl FileTexts {
    pub(crate) fn set(&self, path: &Path, text: &str) {
        self.0.write().insert(path.to_path_buf(), text.into());
    }

    /// The text for a path the checker asked about, matching case only on a
    /// case-sensitive file system.
    fn get(&self, path: &str, case_sensitive: bool) -> Option<Arc<str>> {
        let texts = self.0.read();
        if let Some(text) = texts.get(Path::new(path)) {
            return Some(Arc::clone(text));
        }
        if case_sensitive {
            return None;
        }
        texts
            .iter()
            .find(|(known, _)| known.to_string_lossy().eq_ignore_ascii_case(path))
            .map(|(_, text)| Arc::clone(text))
    }
}

pub(crate) struct TsTypes {
    exe: PathBuf,
    cwd: PathBuf,
    texts: Arc<FileTexts>,
    session: Mutex<Option<Session>>,
    dead: AtomicBool,
}

impl std::fmt::Debug for TsTypes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TsTypes({})", self.exe.display())
    }
}

/// Why no checker is used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoChecker {
    /// Types are not turned on, or `CODEMOD_TS_API=off`.
    Off,
    /// Turned on, but no `tsc` binary was found.
    NotFound,
    /// The binary is not the pinned TypeScript minor.
    WrongVersion { exe: PathBuf, version: String },
}

impl TsTypes {
    /// The checker for a workspace with types turned on (`enabled`):
    /// `CODEMOD_TS_API`, else the workspace's own TypeScript 7 install. Its
    /// version must be [`PINNED_VERSION`].
    pub(crate) fn discover(
        workspace_root: &Path,
        enabled: bool,
        texts: Arc<FileTexts>,
    ) -> Result<Self, NoChecker> {
        if !enabled {
            return Err(NoChecker::Off);
        }
        let exe = match std::env::var("CODEMOD_TS_API") {
            Ok(value) if value == "off" => return Err(NoChecker::Off),
            Ok(value) if !value.is_empty() => PathBuf::from(value),
            _ => installed_checker(workspace_root).ok_or(NoChecker::NotFound)?,
        };
        let version = checker_version(&exe).ok_or(NoChecker::NotFound)?;
        if !version_is_pinned(&version) {
            return Err(NoChecker::WrongVersion { exe, version });
        }
        Ok(Self {
            exe,
            cwd: workspace_root.to_path_buf(),
            texts,
            session: Mutex::new(None),
            dead: AtomicBool::new(false),
        })
    }

    fn with_session<T: Default>(&self, f: impl FnOnce(&mut Session) -> Result<T, String>) -> T {
        if self.dead.load(Ordering::Relaxed) {
            return T::default();
        }
        let mut guard = self.session.lock();
        if guard.is_none() {
            match Session::start(&self.exe, &self.cwd, Arc::clone(&self.texts)) {
                Ok(session) => *guard = Some(session),
                Err(error) => {
                    log::warn!("TypeScript checker did not start: {error}");
                    self.dead.store(true, Ordering::Relaxed);
                    return T::default();
                }
            }
        }
        let session = guard.as_mut().expect("started above");
        match f(session) {
            Ok(value) => value,
            Err(error) => {
                log::debug!("TypeScript checker: {error}");
                if session.broken {
                    log::warn!("TypeScript checker stopped answering: {error}");
                    *guard = None;
                    self.dead.store(true, Ordering::Relaxed);
                }
                T::default()
            }
        }
    }

    /// Round trips and time spent per method so far.
    #[cfg(test)]
    pub(crate) fn stats(&self) -> Vec<(String, u32, std::time::Duration)> {
        let guard = self.session.lock();
        let mut out: Vec<_> = guard
            .iter()
            .flat_map(|s| s.stats.iter())
            .map(|(method, (count, time))| (method.clone(), *count, *time))
            .collect();
        out.sort_by_key(|stat| std::cmp::Reverse(stat.2));
        out
    }

    /// Ask, in one round trip, for the symbols at every byte offset in
    /// `offsets` (the property names of a file's method calls). Later
    /// [`Self::declarations_at`] questions about them are answered from
    /// memory. A file is asked about once.
    pub(crate) fn prefetch(&self, file: &Path, content: &str, offsets: &[u32]) {
        self.with_session(|s| {
            if !s.prefetched.insert(file.to_path_buf()) || offsets.is_empty() {
                return Ok(());
            }
            let Some(project) = s.project_for(file)? else {
                return Ok(());
            };
            let positions: Vec<u32> = offsets.iter().map(|o| utf16_of(content, *o)).collect();
            let symbols = s.request_json(
                "getSymbolsAtPositions",
                &json!({ "snapshot": s.snapshot, "project": project, "file": file, "positions": positions }),
            )?;
            for (offset, symbol) in offsets.iter().zip(symbols.as_array().into_iter().flatten()) {
                s.symbols
                    .insert((file.to_path_buf(), *offset), declaration_handles(symbol));
            }
            Ok(())
        })
    }

    /// Declarations of the symbol at a byte offset: for the property name
    /// of `x.save()`, where `save` is declared on `x`'s type. Library
    /// declarations are included (`library: true`).
    pub(crate) fn declarations_at(&self, file: &Path, content: &str, offset: u32) -> Vec<TsDecl> {
        self.with_session(|s| {
            let Some(project) = s.project_for(file)? else {
                return Ok(Vec::new());
            };
            let key = (file.to_path_buf(), offset);
            let handles = match s.symbols.get(&key) {
                Some(handles) => handles.clone(),
                None => {
                    let position = utf16_of(content, offset);
                    let symbol = s.request_json(
                        "getSymbolAtPosition",
                        &json!({ "snapshot": s.snapshot, "project": project, "file": file, "position": position }),
                    )?;
                    let handles = declaration_handles(&symbol);
                    s.symbols.insert(key, handles.clone());
                    handles
                }
            };
            s.decls(&project, handles)
        })
    }

    /// Declarations that implement or write a signature (an interface's
    /// method -> the class methods that implement it; a property signature
    /// -> the object literals that set it), by the checker's
    /// find-all-references. Workspace declarations only, at most
    /// [`MAX_IMPLEMENTATIONS`].
    pub(crate) fn related_declarations(&self, from: &Path, decl: &TsDecl) -> Related {
        self.with_session(|s| {
            if let Some(found) = s.related.get(&decl.handle) {
                return Ok(found.clone());
            }
            let Some(project) = s.project_for(from)? else {
                return Ok(Related::default());
            };
            // The checker answers for the declaration's name, not the
            // declaration node.
            let Some(name) = s.name_of(&project, &decl.handle)? else {
                return Ok(Related::default());
            };
            let entries = s.request_json(
                "getReferencedSymbolsForNode",
                &json!({ "snapshot": s.snapshot, "project": project, "node": name, "position": 0 }),
            )?;
            // Other definitions (a class method implementing an interface
            // method), and references that write the member: `{ m }`,
            // `{ m: v }`, `m() {}`, `m = v` in an object or class typed by
            // the interface. Those are references to the same symbol, so the
            // checker lists them under the declaration's own entry.
            let mut handles = Vec::new();
            for entry in entries.as_array().into_iter().flatten() {
                if let Some(definition) = entry.get("definition").and_then(Value::as_str) {
                    if definition != decl.handle {
                        handles.push(definition.to_string());
                    }
                }
                for reference in entry
                    .get("references")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    if let Some(parent) = s.member_write_parent(&project, reference)? {
                        if parent != decl.handle {
                            handles.push(parent);
                        }
                    }
                }
            }
            handles.sort();
            handles.dedup();
            let decls: Vec<TsDecl> = s
                .decls(&project, handles)?
                .into_iter()
                .filter(|d| !d.library)
                .collect();
            let found = Related {
                decls: if decls.len() > MAX_IMPLEMENTATIONS {
                    Vec::new()
                } else {
                    decls
                },
            };
            s.related.insert(decl.handle.clone(), found.clone());
            Ok(found)
        })
    }

    /// The type of the expression or declaration name with exactly this
    /// byte range, as the checker prints it.
    pub(crate) fn type_at(&self, file: &Path, content: &str, range: ByteRange) -> Option<String> {
        self.with_session(|s| {
            let Some(project) = s.project_for(file)? else {
                return Ok(None);
            };
            let Some(ast) = s.source_file(&project, &file.to_string_lossy())? else {
                return Ok(None);
            };
            let Some((index, kind)) = ast.node_with_range(content, range) else {
                return Ok(None);
            };
            let handle = format!("{index}.{kind}.{}", s.server_path(file));
            let ty = s.request_json(
                "getTypeAtLocation",
                &json!({ "snapshot": s.snapshot, "project": project, "location": handle }),
            )?;
            let Some(id) = ty.get("id").and_then(Value::as_u64) else {
                return Ok(None);
            };
            let text = s.request_json(
                "typeToString",
                &json!({ "snapshot": s.snapshot, "project": project, "type": id, "location": handle }),
            )?;
            Ok(text.as_str().map(str::to_string))
        })
    }
}

struct Session {
    child: Child,
    texts: Arc<FileTexts>,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    case_sensitive: bool,
    snapshot: u64,
    /// File -> its project's id (None: no project).
    file_project: HashMap<PathBuf, Option<String>>,
    /// Server path of a project's root file -> the project's id.
    root_owner: HashMap<String, String>,
    /// Server path -> the path as the workspace spells it.
    real_paths: HashMap<String, PathBuf>,
    /// Binary ASTs by server path. Files are not edited during a session.
    asts: HashMap<String, Arc<BinaryAst>>,
    related: HashMap<String, Related>,
    /// Declaration handles of the symbol at (file, byte offset).
    symbols: HashMap<(PathBuf, u32), Vec<String>>,
    /// Files whose method calls were asked about in one batch.
    prefetched: HashSet<PathBuf>,
    /// Round trips and time spent, by method (for the corpus metric).
    stats: HashMap<String, (u32, std::time::Duration)>,
    broken: bool,
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Session {
    fn start(exe: &Path, cwd: &Path, texts: Arc<FileTexts>) -> Result<Self, String> {
        let mut child = Command::new(exe)
            .arg("--api")
            .arg("--cwd")
            .arg(cwd)
            .arg("--callbacks")
            .arg("readFile,fileExists")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // The CLI owns the terminal: nothing from the checker reaches it.
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("spawn {}: {e}", exe.display()))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = BufReader::with_capacity(1 << 16, child.stdout.take().ok_or("no stdout")?);
        let mut session = Self {
            child,
            texts,
            stdin,
            stdout,
            case_sensitive: true,
            snapshot: 0,
            file_project: HashMap::new(),
            root_owner: HashMap::new(),
            real_paths: HashMap::new(),
            asts: HashMap::new(),
            related: HashMap::new(),
            symbols: HashMap::new(),
            prefetched: HashSet::new(),
            stats: HashMap::new(),
            broken: false,
        };
        let init = session.request_json("initialize", &Value::Null)?;
        session.case_sensitive = init
            .get("useCaseSensitiveFileNames")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        Ok(session)
    }

    fn server_path(&self, path: &Path) -> String {
        let text = path.to_string_lossy().replace('\\', "/");
        if self.case_sensitive {
            text
        } else {
            text.to_lowercase()
        }
    }

    fn real_path(&mut self, server_path: &str) -> PathBuf {
        if let Some(real) = self.real_paths.get(server_path) {
            return real.clone();
        }
        let real = if self.case_sensitive {
            PathBuf::from(server_path)
        } else {
            recover_case(Path::new(server_path))
        };
        self.real_paths
            .insert(server_path.to_string(), real.clone());
        real
    }

    /// The project a file belongs to: a project already loaded that lists
    /// it, else the one the checker opens for it (nearest tsconfig, or an
    /// inferred project).
    fn project_for(&mut self, file: &Path) -> Result<Option<String>, String> {
        if let Some(project) = self.file_project.get(file) {
            return Ok(project.clone());
        }
        let key = self.server_path(file);
        if let Some(project) = self.root_owner.get(&key) {
            let project = Some(project.clone());
            self.file_project
                .insert(file.to_path_buf(), project.clone());
            return Ok(project);
        }
        let previous = self.snapshot;
        let snapshot = self.request_json("updateSnapshot", &json!({ "openFiles": [file] }))?;
        self.snapshot = snapshot
            .get("snapshot")
            .and_then(Value::as_u64)
            .ok_or("updateSnapshot: no snapshot")?;
        if previous != 0 {
            let _ = self.request_json("release", &json!({ "snapshot": previous }));
        }
        for project in snapshot
            .get("projects")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(id) = project.get("id").and_then(Value::as_str) else {
                continue;
            };
            for root in project
                .get("rootFiles")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                let server = self.server_path(Path::new(root));
                self.real_paths.insert(server.clone(), PathBuf::from(root));
                self.root_owner
                    .entry(server)
                    .or_insert_with(|| id.to_string());
            }
        }
        let default = self.request_json(
            "getDefaultProjectForFile",
            &json!({ "snapshot": self.snapshot, "file": file }),
        )?;
        let project = default
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string);
        self.real_paths.insert(key, file.to_path_buf());
        self.file_project
            .insert(file.to_path_buf(), project.clone());
        Ok(project)
    }

    fn source_file(&mut self, project: &str, path: &str) -> Result<Option<Arc<BinaryAst>>, String> {
        let key = if self.case_sensitive {
            path.to_string()
        } else {
            path.to_lowercase()
        };
        if let Some(ast) = self.asts.get(&key) {
            return Ok(Some(Arc::clone(ast)));
        }
        let data = self.request(
            "getSourceFile",
            json!({ "snapshot": self.snapshot, "project": project, "file": path }).to_string(),
        )?;
        if data.len() < HEADER_OFFSET_NODES + 4 {
            return Ok(None);
        }
        let ast = Arc::new(BinaryAst { data });
        self.asts.insert(key, Arc::clone(&ast));
        Ok(Some(ast))
    }

    /// (index, pos16, end16) of a handle's node.
    fn node(&mut self, project: &str, handle: &str) -> Result<Option<(u32, u32, u32)>, String> {
        let Some((index, _, path)) = parse_handle(handle) else {
            return Ok(None);
        };
        let real = self.real_path(path).to_string_lossy().to_string();
        let Some(ast) = self.source_file(project, &real)? else {
            return Ok(None);
        };
        Ok(ast.pos_end(index).map(|(pos, end)| (index, pos, end)))
    }

    /// The handle of a declaration's name identifier.
    fn name_of(&mut self, project: &str, handle: &str) -> Result<Option<String>, String> {
        let Some((index, _, path)) = parse_handle(handle) else {
            return Ok(None);
        };
        let real = self.real_path(path).to_string_lossy().to_string();
        let Some(ast) = self.source_file(project, &real)? else {
            return Ok(None);
        };
        Ok(ast
            .name_child(index)
            .map(|(child, kind)| format!("{child}.{kind}.{path}")))
    }

    /// The handle of a reference's parent when the parent writes a member:
    /// a property assignment (`m: v`, `{ m }`), method, field or accessor.
    fn member_write_parent(
        &mut self,
        project: &str,
        reference: &str,
    ) -> Result<Option<String>, String> {
        let Some((index, _, path)) = parse_handle(reference) else {
            return Ok(None);
        };
        if path.contains("/node_modules/") || path.ends_with(".d.ts") {
            return Ok(None);
        }
        let real = self.real_path(path).to_string_lossy().to_string();
        let Some(ast) = self.source_file(project, &real)? else {
            return Ok(None);
        };
        let Some(parent) = ast.parent(index) else {
            return Ok(None);
        };
        let Some((kind, _, _)) = ast.record(parent as usize) else {
            return Ok(None);
        };
        Ok(MEMBER_WRITE_KINDS
            .contains(&kind)
            .then(|| format!("{parent}.{kind}.{path}")))
    }

    /// Declarations for node handles, with library ones (`node_modules`,
    /// `.d.ts`) marked.
    fn decls(&mut self, project: &str, handles: Vec<String>) -> Result<Vec<TsDecl>, String> {
        let mut out = Vec::new();
        for handle in handles {
            let Some((_, _, path)) = parse_handle(&handle) else {
                continue;
            };
            let library = path.contains("/node_modules/") || path.ends_with(".d.ts");
            let real = self.real_path(path);
            if let Some((_, pos16, end16)) = self.node(project, &handle)? {
                out.push(TsDecl {
                    path: real,
                    pos16,
                    end16,
                    library,
                    handle,
                });
            }
        }
        Ok(out)
    }

    fn request_json(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        let bytes = self.request(method, params.to_string())?;
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes).map_err(|e| format!("{method}: {e}"))
    }

    fn request(&mut self, method: &str, payload: String) -> Result<Vec<u8>, String> {
        let started = std::time::Instant::now();
        let result = self.exchange(method, payload.as_bytes());
        let stat = self.stats.entry(method.to_string()).or_default();
        stat.0 += 1;
        stat.1 += started.elapsed();
        if let Err(error) = &result {
            if error.starts_with("io:") {
                self.broken = true;
            }
        }
        result
    }

    fn exchange(&mut self, method: &str, payload: &[u8]) -> Result<Vec<u8>, String> {
        let io = |e: std::io::Error| format!("io: {e}");
        let mut message = Vec::with_capacity(payload.len() + method.len() + 12);
        message.push(0x93);
        message.push(MSG_REQUEST);
        write_bin(&mut message, method.as_bytes());
        write_bin(&mut message, payload);
        self.stdin.write_all(&message).map_err(io)?;
        self.stdin.flush().map_err(io)?;
        loop {
            let mut head = [0u8; 2];
            self.stdout.read_exact(&mut head).map_err(io)?;
            if head[0] != 0x93 {
                return Err(format!("io: bad frame 0x{:x}", head[0]));
            }
            let kind = if head[1] == 0xcc {
                let mut b = [0u8; 1];
                self.stdout.read_exact(&mut b).map_err(io)?;
                b[0]
            } else {
                head[1]
            };
            let name = read_bin(&mut self.stdout).map_err(io)?;
            let body = read_bin(&mut self.stdout).map_err(io)?;
            match kind {
                MSG_RESPONSE if name == method.as_bytes() => return Ok(body),
                MSG_ERROR => {
                    return Err(format!("{method}: {}", String::from_utf8_lossy(&body)));
                }
                MSG_CALL => {
                    let reply = self.answer_call(&name, &body);
                    self.stdin.write_all(&reply).map_err(io)?;
                    self.stdin.flush().map_err(io)?;
                }
                _ => return Err(format!("io: unexpected message {kind} for {method}")),
            }
        }
    }
}

impl Session {
    /// The checker asks for a file: its text from [`FileTexts`], or `null`,
    /// which tells the checker to read the disk itself.
    fn answer_call(&self, name: &[u8], body: &[u8]) -> Vec<u8> {
        let text = serde_json::from_slice::<String>(body)
            .ok()
            .and_then(|path| self.texts.get(&path, self.case_sensitive));
        let answer = match (name, text) {
            (b"readFile", Some(text)) => json!({ "content": &*text }),
            (b"fileExists", Some(_)) => json!(true),
            (b"readFile" | b"fileExists", None) => Value::Null,
            _ => {
                let mut reply = vec![0x93, MSG_CALL_ERROR];
                write_bin(&mut reply, name);
                write_bin(&mut reply, b"unknown callback");
                return reply;
            }
        };
        let mut reply = vec![0x93, MSG_CALL_RESPONSE];
        write_bin(&mut reply, name);
        write_bin(&mut reply, answer.to_string().as_bytes());
        reply
    }
}

/// The checker lowercases paths on a case-insensitive file system. The
/// spelling on disk, component by component; as given where a directory
/// cannot be read.
fn recover_case(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        let name = component.as_os_str();
        let found = std::fs::read_dir(if out.as_os_str().is_empty() {
            Path::new(".")
        } else {
            &out
        })
        .ok()
        .and_then(|entries| {
            entries.flatten().map(|e| e.file_name()).find(|entry| {
                entry.to_string_lossy().to_lowercase() == name.to_string_lossy().to_lowercase()
            })
        });
        match (component, found) {
            (std::path::Component::Normal(_), Some(entry)) => out.push(entry),
            _ => out.push(name),
        }
    }
    out
}

fn write_bin(out: &mut Vec<u8>, bytes: &[u8]) {
    let len = bytes.len();
    if len < 256 {
        out.extend_from_slice(&[0xc4, len as u8]);
    } else if len < 65536 {
        out.push(0xc5);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0xc6);
        out.extend_from_slice(&(len as u32).to_be_bytes());
    }
    out.extend_from_slice(bytes);
}

fn read_bin(input: &mut impl Read) -> std::io::Result<Vec<u8>> {
    let mut marker = [0u8; 1];
    input.read_exact(&mut marker)?;
    let len = match marker[0] {
        0xc4 => {
            let mut b = [0u8; 1];
            input.read_exact(&mut b)?;
            b[0] as usize
        }
        0xc5 => {
            let mut b = [0u8; 2];
            input.read_exact(&mut b)?;
            u16::from_be_bytes(b) as usize
        }
        0xc6 => {
            let mut b = [0u8; 4];
            input.read_exact(&mut b)?;
            u32::from_be_bytes(b) as usize
        }
        other => {
            return Err(std::io::Error::other(format!("bad bin marker 0x{other:x}")));
        }
    };
    let mut body = vec![0u8; len];
    input.read_exact(&mut body)?;
    Ok(body)
}

/// `index.kind.path`.
fn parse_handle(handle: &str) -> Option<(u32, u32, &str)> {
    let (index, rest) = handle.split_once('.')?;
    let (kind, path) = rest.split_once('.')?;
    Some((index.parse().ok()?, kind.parse().ok()?, path))
}

/// A file's AST as the checker sends it: a flat array of 28-byte nodes in
/// pre-order (`kind`, `pos`, `end`, `next`, `parent`, `data`, `flags`).
struct BinaryAst {
    data: Vec<u8>,
}

impl BinaryAst {
    fn u32_at(&self, offset: usize) -> Option<u32> {
        let bytes = self.data.get(offset..offset + 4)?;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    }

    fn nodes_offset(&self) -> usize {
        self.u32_at(HEADER_OFFSET_NODES).unwrap_or(0) as usize
    }

    fn node_count(&self) -> usize {
        self.data.len().saturating_sub(self.nodes_offset()) / NODE_LEN
    }

    fn record(&self, index: usize) -> Option<(u32, u32, u32)> {
        let base = self.nodes_offset() + index * NODE_LEN;
        Some((
            self.u32_at(base)?,
            self.u32_at(base + 4)?,
            self.u32_at(base + 8)?,
        ))
    }

    /// The first identifier child of a node (a declaration's name).
    fn name_child(&self, index: u32) -> Option<(u32, u32)> {
        let (_, _, end) = self.record(index as usize)?;
        for child in index as usize + 1..self.node_count() {
            let (kind, pos, _) = self.record(child)?;
            if kind != KIND_NODE_LIST && pos >= end {
                return None;
            }
            if (kind == KIND_IDENTIFIER || kind == KIND_PRIVATE_IDENTIFIER)
                && self.parent(child as u32) == Some(index)
            {
                return Some((child as u32, kind));
            }
        }
        None
    }

    fn parent(&self, index: u32) -> Option<u32> {
        let mut parent = self.u32_at(self.nodes_offset() + index as usize * NODE_LEN + 16)?;
        // Lists sit between a node and its children.
        while self.record(parent as usize)?.0 == KIND_NODE_LIST {
            parent = self.u32_at(self.nodes_offset() + parent as usize * NODE_LEN + 16)?;
        }
        Some(parent)
    }

    fn pos_end(&self, index: u32) -> Option<(u32, u32)> {
        let (_, pos, end) = self.record(index as usize)?;
        Some((pos, end))
    }

    /// The deepest node whose range, without leading trivia, is exactly
    /// range (bytes of content).
    fn node_with_range(&self, content: &str, range: ByteRange) -> Option<(usize, u32)> {
        let start16 = utf16_of(content, range.start);
        let end16 = utf16_of(content, range.end);
        let mut found = None;
        for index in 1..self.node_count() {
            let Some((kind, pos, end)) = self.record(index) else {
                break;
            };
            if kind == KIND_NODE_LIST || end != end16 || pos > start16 {
                continue;
            }
            let start = skip_trivia(content, byte_of(content, pos));
            if start == range.start {
                found = Some((index, kind));
            }
        }
        found
    }
}

/// UTF-16 offset of a byte offset.
pub(crate) fn utf16_of(content: &str, byte: u32) -> u32 {
    let byte = (byte as usize).min(content.len());
    if content.is_ascii() {
        return byte as u32;
    }
    content
        .get(..byte)
        .map(|s| s.encode_utf16().count() as u32)
        .unwrap_or(byte as u32)
}

/// Byte offset of a UTF-16 offset.
pub(crate) fn byte_of(content: &str, utf16: u32) -> u32 {
    if content.is_ascii() {
        return utf16.min(content.len() as u32);
    }
    let mut units = 0u32;
    for (index, ch) in content.char_indices() {
        if units >= utf16 {
            return index as u32;
        }
        units += ch.len_utf16() as u32;
    }
    content.len() as u32
}

/// The first byte at or after `from` that is not whitespace or a comment.
pub(crate) fn skip_trivia(content: &str, from: u32) -> u32 {
    let bytes = content.as_bytes();
    let mut i = from as usize;
    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c => i += 1,
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            _ if content[i..].starts_with('\u{feff}') => i += 3,
            _ => break,
        }
    }
    i.min(bytes.len()) as u32
}

fn declaration_handles(symbol: &Value) -> Vec<String> {
    symbol
        .get("declarations")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|h| h.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The workspace's own TypeScript 7 binary, if installed.
fn installed_checker(workspace_root: &Path) -> Option<PathBuf> {
    let platform = format!("{}-{}", node_os(), node_arch());
    [
        format!("node_modules/@typescript/typescript-{platform}/lib/tsc"),
        format!("node_modules/@typescript/native-preview-{platform}/lib/tsgo"),
    ]
    .iter()
    .map(|candidate| workspace_root.join(candidate))
    .find(|path| path.is_file())
}

/// `tsc --version`: `Version 7.0.2` -> `7.0.2`.
fn checker_version(exe: &Path) -> Option<String> {
    let output = Command::new(exe)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let version = text.trim().strip_prefix("Version ")?.trim();
    Some(version.to_string())
}

fn version_is_pinned(version: &str) -> bool {
    version
        .strip_prefix(PINNED_VERSION)
        .is_some_and(|rest| rest.starts_with('.'))
}

fn node_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

fn node_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_round_trip_through_utf16() {
        let text = "// é\nconst x = \"😀\"; x";
        let x = text.rfind('x').unwrap() as u32;
        let u16 = utf16_of(text, x);
        assert_eq!(u16, x - 1 - 2);
        assert_eq!(byte_of(text, u16), x);
    }

    #[test]
    fn trivia_is_skipped() {
        let text = "a  /* c */ // d\n  b";
        assert_eq!(skip_trivia(text, 1), text.find('b').unwrap() as u32);
    }

    #[test]
    fn only_the_pinned_minor_is_used() {
        assert!(version_is_pinned("7.0.2"));
        assert!(version_is_pinned("7.0.10-dev.1"));
        assert!(!version_is_pinned("7.1.0"));
        assert!(!version_is_pinned("7.00.1"));
        assert!(!version_is_pinned("6.0.3"));
    }

    /// Protocol smoke test against a real checker (CI sets CODEMOD_TS_API
    /// to the pinned TypeScript): types, declarations, and the batch.
    #[test]
    fn protocol_smoke_test() {
        if std::env::var_os("CODEMOD_TS_API").is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let file = root.join("a.ts");
        let content = "// é\nclass C { m() {} }\nconst c: C = new C();\nc.m();\n";
        std::fs::write(&file, content).unwrap();
        let types = TsTypes::discover(&root, true, Default::default()).expect("the pinned checker");
        let c = content.rfind("c.m").unwrap() as u32;
        assert_eq!(
            types
                .type_at(&file, content, ByteRange::new(c, c + 1))
                .as_deref(),
            Some("C")
        );
        types.prefetch(&file, content, &[c + 2]);
        let decls = types.declarations_at(&file, content, c + 2);
        assert_eq!(decls.len(), 1);
        let start = skip_trivia(content, byte_of(content, decls[0].pos16)) as usize;
        let end = byte_of(content, decls[0].end16) as usize;
        assert_eq!(&content[start..end], "m() {}");
        assert!(!decls[0].library);
    }

    #[test]
    fn handles_parse() {
        assert_eq!(
            parse_handle("12.175./users/x/a.ts"),
            Some((12, 175, "/users/x/a.ts"))
        );
    }
}
