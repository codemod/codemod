//! `globalThis.__codemodQuickjsNative`: the primitives the bundle's QuickJS
//! `Host` (`packages/orchestration/src/host/quickjs/host.ts`) is built from.
//! Everything here is synchronous except `spawn`, whose result is a promise
//! settled by a future the context drives. Errors are `Error`s carrying a
//! POSIX `code` string (`ENOENT`, `EILSEQ`, ...), as `core/host.ts` requires.
//!
//! `print` is the workflow's console. This binary is a terminal entry point
//! (like `codemod-workflow`), so console output goes to stderr and stdout
//! stays the JSON result.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::thread::JoinHandle;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rquickjs::prelude::Opt;
use rquickjs::{Ctx, Exception, Function, Object, Promise, Value};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use crate::jssg;
use crate::modules::ModuleRegistry;
use crate::select::{self, SelectRequest};
use crate::utf8::StreamDecoder;

/// The global the bundle reads its primitives from.
pub const NATIVE_GLOBAL: &str = "__codemodQuickjsNative";

pub struct HostState {
    pub started: Instant,
    pub registry: ModuleRegistry,
    pub batches: RefCell<HashMap<u32, jssg::OpenBatch>>,
    /// Live children by spawn id: their process id (= process group id).
    pub children: RefCell<HashMap<u32, u32>>,
    pub next_id: Cell<u32>,
    pub stats: RefCell<jssg::Stats>,
}

impl HostState {
    pub fn next_id(&self) -> u32 {
        let id = self.next_id.get() + 1;
        self.next_id.set(id);
        id
    }
}

/// Throw an `Error` with `message` and, when given, a `code` property.
pub fn throw<'js>(ctx: &Ctx<'js>, code: Option<&str>, message: &str) -> rquickjs::Error {
    match error_value(ctx, code, message) {
        Ok(value) => ctx.throw(value),
        Err(error) => error,
    }
}

fn error_value<'js>(
    ctx: &Ctx<'js>,
    code: Option<&str>,
    message: &str,
) -> rquickjs::Result<Value<'js>> {
    let exception = Exception::from_message(ctx.clone(), message)?;
    if let Some(code) = code {
        exception.as_object().set("code", code)?;
    }
    Ok(exception.into_value())
}

/// POSIX name of an I/O error, as Node reports it in `error.code`.
pub fn io_code(error: &io::Error) -> &'static str {
    if let Some(errno) = error.raw_os_error() {
        return match errno {
            libc::ENOENT => "ENOENT",
            libc::EEXIST => "EEXIST",
            libc::ENOTDIR => "ENOTDIR",
            libc::EISDIR => "EISDIR",
            libc::EACCES => "EACCES",
            libc::EPERM => "EPERM",
            libc::ENOTEMPTY => "ENOTEMPTY",
            libc::ELOOP => "ELOOP",
            libc::EINVAL => "EINVAL",
            libc::EBUSY => "EBUSY",
            libc::EMFILE => "EMFILE",
            libc::ENAMETOOLONG => "ENAMETOOLONG",
            libc::EROFS => "EROFS",
            libc::EXDEV => "EXDEV",
            _ => "EIO",
        };
    }
    match error.kind() {
        io::ErrorKind::NotFound => "ENOENT",
        io::ErrorKind::AlreadyExists => "EEXIST",
        io::ErrorKind::PermissionDenied => "EACCES",
        io::ErrorKind::NotADirectory => "ENOTDIR",
        io::ErrorKind::IsADirectory => "EISDIR",
        io::ErrorKind::DirectoryNotEmpty => "ENOTEMPTY",
        io::ErrorKind::InvalidInput => "EINVAL",
        _ => "EIO",
    }
}

fn fs_error(ctx: &Ctx<'_>, op: &str, path: &str, error: io::Error) -> rquickjs::Error {
    let code = io_code(&error);
    throw(ctx, Some(code), &format!("{code}: {error}, {op} '{path}'"))
}

/// Walker failures as the TypeScript walker over this host's `fs` reports them.
fn walker_error(ctx: &Ctx<'_>, error: orchestration_walker::Error) -> rquickjs::Error {
    let (code, message) = select::describe(&error);
    throw(ctx, code, &message)
}

/// Node's `os.platform()` names.
fn platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

fn total_memory() -> f64 {
    // SAFETY: sysconf has no preconditions.
    let (pages, size) = unsafe {
        (
            libc::sysconf(libc::_SC_PHYS_PAGES),
            libc::sysconf(libc::_SC_PAGESIZE),
        )
    };
    if pages > 0 && size > 0 {
        pages as f64 * size as f64
    } else {
        0.0
    }
}

fn tmpdir() -> String {
    let dir = std::env::temp_dir().to_string_lossy().into_owned();
    // Node's os.tmpdir() strips a trailing separator.
    if dir.len() > 1 {
        dir.trim_end_matches('/').to_string()
    } else {
        dir
    }
}

fn signal_name(signal: i32) -> String {
    match signal {
        libc::SIGHUP => "SIGHUP",
        libc::SIGINT => "SIGINT",
        libc::SIGQUIT => "SIGQUIT",
        libc::SIGILL => "SIGILL",
        libc::SIGABRT => "SIGABRT",
        libc::SIGKILL => "SIGKILL",
        libc::SIGSEGV => "SIGSEGV",
        libc::SIGPIPE => "SIGPIPE",
        libc::SIGALRM => "SIGALRM",
        libc::SIGTERM => "SIGTERM",
        libc::SIGUSR1 => "SIGUSR1",
        libc::SIGUSR2 => "SIGUSR2",
        libc::SIGBUS => "SIGBUS",
        _ => return format!("SIG{signal}"),
    }
    .to_string()
}

pub fn install<'js>(ctx: &Ctx<'js>, state: Rc<HostState>) -> rquickjs::Result<()> {
    let native = Object::new(ctx.clone())?;

    // --- fs -----------------------------------------------------------------
    native.set(
        "readText",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, path: String, fatal: Opt<bool>| {
                let bytes =
                    fs::read(&path).map_err(|error| fs_error(&ctx, "open", &path, error))?;
                if fatal.0.unwrap_or(false) {
                    String::from_utf8(bytes).map_err(|_| {
                        throw(
                            &ctx,
                            Some("EILSEQ"),
                            &format!("'{path}' is not valid UTF-8"),
                        )
                    })
                } else {
                    Ok(match String::from_utf8(bytes) {
                        Ok(text) => text,
                        Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
                    })
                }
            },
        )?,
    )?;
    native.set(
        "writeText",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, path: String, content: String, mode: Opt<Option<u32>>| {
                let mut options = fs::OpenOptions::new();
                options.write(true).create(true).truncate(true);
                if let Some(mode) = mode.0.flatten() {
                    options.mode(mode);
                }
                options
                    .open(&path)
                    .and_then(|mut file| file.write_all(content.as_bytes()))
                    .map_err(|error| fs_error(&ctx, "open", &path, error))
            },
        )?,
    )?;
    native.set(
        "mkdir",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, path: String, recursive: Opt<bool>| {
                let result = if recursive.0.unwrap_or(false) {
                    fs::create_dir_all(&path)
                } else {
                    fs::create_dir(&path)
                };
                result.map_err(|error| fs_error(&ctx, "mkdir", &path, error))
            },
        )?,
    )?;
    native.set(
        "rm",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, path: String, force: Opt<bool>, recursive: Opt<bool>| {
                let force = force.0.unwrap_or(false);
                let result = match fs::symlink_metadata(&path) {
                    Err(error) if force && error.kind() == io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(error),
                    Ok(meta) if meta.is_dir() => {
                        if recursive.0.unwrap_or(false) {
                            fs::remove_dir_all(&path)
                        } else {
                            return Err(throw(
                                &ctx,
                                Some("EISDIR"),
                                &format!("EISDIR: is a directory, rm '{path}'"),
                            ));
                        }
                    }
                    Ok(_) => fs::remove_file(&path),
                };
                match result {
                    Err(error) if force && error.kind() == io::ErrorKind::NotFound => Ok(()),
                    other => other.map_err(|error| fs_error(&ctx, "rm", &path, error)),
                }
            },
        )?,
    )?;
    native.set(
        "lstat",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, path: String| {
            let meta = fs::symlink_metadata(&path)
                .map_err(|error| fs_error(&ctx, "lstat", &path, error))?;
            let stat = Object::new(ctx.clone())?;
            let kind = meta.file_type();
            stat.set("isFile", kind.is_file())?;
            stat.set("isDirectory", kind.is_dir())?;
            stat.set("isSymbolicLink", kind.is_symlink())?;
            Ok::<_, rquickjs::Error>(stat)
        })?,
    )?;
    native.set(
        "realpath",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, path: String| {
            fs::canonicalize(&path)
                .map(|real| real.to_string_lossy().into_owned())
                .map_err(|error| fs_error(&ctx, "realpath", &path, error))
        })?,
    )?;
    native.set(
        "readdir",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, path: String| {
            let entries =
                fs::read_dir(&path).map_err(|error| fs_error(&ctx, "scandir", &path, error))?;
            let list = rquickjs::Array::new(ctx.clone())?;
            for (index, entry) in entries.enumerate() {
                let entry = entry.map_err(|error| fs_error(&ctx, "scandir", &path, error))?;
                let kind = entry
                    .file_type()
                    .map_err(|error| fs_error(&ctx, "scandir", &path, error))?;
                let item = Object::new(ctx.clone())?;
                item.set("name", entry.file_name().to_string_lossy().as_ref())?;
                item.set("isFile", kind.is_file())?;
                item.set("isDirectory", kind.is_dir())?;
                list.set(index, item)?;
            }
            Ok::<_, rquickjs::Error>(list)
        })?,
    )?;

    // --- file selection (`Host.selectFiles`, `Host.fs.readSources`) ---------
    native.set(
        "selectFiles",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, request: String| {
            let request: SelectRequest = serde_json::from_str(&request).map_err(|error| {
                throw(&ctx, None, &format!("invalid selectFiles request: {error}"))
            })?;
            orchestration_walker::select(&request.into()).map_err(|error| walker_error(&ctx, error))
        })?,
    )?;
    native.set(
        "readSources",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, root: String, paths: Vec<String>| {
                orchestration_walker::read_sources(&root, &paths)
                    .map_err(|error| walker_error(&ctx, error))
            },
        )?,
    )?;

    // --- process facts ------------------------------------------------------
    native.set("platform", platform())?;
    native.set(
        "availableParallelism",
        Function::new(ctx.clone(), || {
            std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(1)
        })?,
    )?;
    native.set("totalmem", Function::new(ctx.clone(), total_memory)?)?;
    native.set(
        "homedir",
        Function::new(ctx.clone(), || {
            std::env::var("HOME").ok().filter(|h| !h.is_empty())
        })?,
    )?;
    native.set("tmpdir", Function::new(ctx.clone(), tmpdir)?)?;
    native.set(
        "cwd",
        Function::new(ctx.clone(), |ctx: Ctx<'js>| {
            std::env::current_dir()
                .map(|dir| dir.to_string_lossy().into_owned())
                .map_err(|error| fs_error(&ctx, "uv_cwd", ".", error))
        })?,
    )?;
    native.set(
        "env",
        Function::new(ctx.clone(), |ctx: Ctx<'js>| {
            let env = Object::new(ctx.clone())?;
            for (name, value) in std::env::vars_os() {
                env.set(
                    name.to_string_lossy().as_ref(),
                    value.to_string_lossy().as_ref(),
                )?;
            }
            Ok::<_, rquickjs::Error>(env)
        })?,
    )?;
    native.set(
        "now",
        Function::new(ctx.clone(), || {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_millis() as f64)
                .unwrap_or(0.0)
        })?,
    )?;
    {
        let state = Rc::clone(&state);
        native.set(
            "hrtime",
            Function::new(ctx.clone(), move || {
                state.started.elapsed().as_secs_f64() * 1000.0
            })?,
        )?;
    }
    native.set(
        "randomUUID",
        Function::new(ctx.clone(), || uuid::Uuid::new_v4().to_string())?,
    )?;
    native.set(
        "print",
        Function::new(ctx.clone(), |_stream: String, text: String| {
            // Console output never goes to stdout (the JSON result).
            let mut stderr = io::stderr().lock();
            let _ = stderr.write_all(text.as_bytes());
        })?,
    )?;

    // --- modules ------------------------------------------------------------
    {
        let state = Rc::clone(&state);
        native.set(
            "registerModule",
            Function::new(ctx.clone(), move |specifier: String, source: String| {
                state.registry.register(specifier, source);
            })?,
        )?;
    }

    // --- processes ----------------------------------------------------------
    {
        let state = Rc::clone(&state);
        native.set(
            "spawn",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>,
                      file: String,
                      args: Vec<String>,
                      cwd: String,
                      env: HashMap<String, String>,
                      options: Object<'js>| {
                    let stdin: Option<String> = options.get("stdin")?;
                    let stderr: Option<String> = options.get("stderr")?;
                    let on_output: Option<Function<'js>> = options.get("onOutput")?;
                    spawn(
                        &ctx,
                        &state,
                        file,
                        args,
                        cwd,
                        env,
                        stdin,
                        stderr.as_deref().unwrap_or("pipe"),
                        on_output,
                    )
                },
            )?,
        )?;
    }
    {
        let state = Rc::clone(&state);
        native.set(
            "kill",
            Function::new(ctx.clone(), move |id: u32| {
                if let Some(pid) = state.children.borrow().get(&id).copied() {
                    // The child leads its own process group: kill the group,
                    // then the child itself in case it left it.
                    // SAFETY: plain signal delivery to a pid we spawned.
                    unsafe {
                        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
                        libc::kill(pid as libc::pid_t, libc::SIGKILL);
                    }
                }
            })?,
        )?;
    }

    jssg::install(ctx, &native, Rc::clone(&state))?;
    ctx.globals().set(NATIVE_GLOBAL, native)?;
    Ok(())
}

enum ChildEvent {
    Output(&'static str, String),
    Exit {
        code: Option<i32>,
        signal: Option<String>,
    },
}

fn pump(
    mut reader: impl Read + Send + 'static,
    stream: &'static str,
    tx: UnboundedSender<ChildEvent>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut decoder = StreamDecoder::default();
        let mut buffer = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    let text = decoder.push(&buffer[..n]);
                    if !text.is_empty() && tx.send(ChildEvent::Output(stream, text)).is_err() {
                        break;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        let tail = decoder.finish();
        if !tail.is_empty() {
            let _ = tx.send(ChildEvent::Output(stream, tail));
        }
    })
}

/// `{ id, pid, result }`. Never throws for a start failure: `result` rejects
/// with the error (and its `code`) instead, as `Host.spawn` requires.
#[allow(clippy::too_many_arguments)]
fn spawn<'js>(
    ctx: &Ctx<'js>,
    state: &Rc<HostState>,
    file: String,
    args: Vec<String>,
    cwd: String,
    env: HashMap<String, String>,
    stdin: Option<String>,
    stderr_mode: &str,
    on_output: Option<Function<'js>>,
) -> rquickjs::Result<Object<'js>> {
    let (promise, resolve, reject) = Promise::new(ctx)?;
    let handle = Object::new(ctx.clone())?;
    let id = state.next_id();
    handle.set("id", id)?;
    handle.set("result", promise)?;

    let mut command = Command::new(&file);
    command
        .args(&args)
        .current_dir(&cwd)
        .env_clear()
        .envs(&env)
        .process_group(0)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    let mut merged = None;
    match stderr_mode {
        "merge" => match io::pipe().and_then(|(reader, writer)| {
            let copy = writer.try_clone()?;
            Ok((reader, writer, copy))
        }) {
            Ok((reader, writer, copy)) => {
                command.stdout(writer).stderr(copy);
                merged = Some(reader);
            }
            Err(error) => {
                let code = io_code(&error);
                reject.call::<_, ()>((error_value(
                    ctx,
                    Some(code),
                    &format!("spawn {file} {code}: {error}"),
                )?,))?;
                return Ok(handle);
            }
        },
        "ignore" => {
            command.stdout(Stdio::piped()).stderr(Stdio::null());
        }
        _ => {
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
        }
    }
    let spawned = command.spawn();
    // Drop the parent's copies of the pipe's write ends, or the readers never see EOF.
    drop(command);
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            let code = io_code(&error);
            reject.call::<_, ()>((error_value(
                ctx,
                Some(code),
                &format!("spawn {file} {code}: {error}"),
            )?,))?;
            return Ok(handle);
        }
    };
    let pid = child.id();
    handle.set("pid", pid)?;
    state.children.borrow_mut().insert(id, pid);

    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        std::thread::spawn(move || {
            let _ = pipe.write_all(text.as_bytes());
        });
    }
    let (tx, mut rx) = unbounded_channel();
    let mut pumps = Vec::new();
    if let Some(reader) = merged {
        pumps.push(pump(reader, "stdout", tx.clone()));
    }
    if let Some(stdout) = child.stdout.take() {
        pumps.push(pump(stdout, "stdout", tx.clone()));
    }
    if let Some(stderr) = child.stderr.take() {
        pumps.push(pump(stderr, "stderr", tx.clone()));
    }
    std::thread::spawn(move || {
        // Like Node's `close`: the output is drained before the exit is reported.
        for pump in pumps {
            let _ = pump.join();
        }
        let (code, signal) = match child.wait() {
            Ok(status) => (status.code(), status.signal().map(signal_name)),
            Err(_) => (None, None),
        };
        let _ = tx.send(ChildEvent::Exit { code, signal });
    });

    let ctx2 = ctx.clone();
    let state = Rc::clone(state);
    ctx.spawn(async move {
        let mut stdout = String::new();
        let mut stderr = String::new();
        while let Some(event) = rx.recv().await {
            match event {
                ChildEvent::Output(stream, chunk) => {
                    if stream == "stdout" {
                        stdout.push_str(&chunk);
                    } else {
                        stderr.push_str(&chunk);
                    }
                    if let Some(callback) = &on_output {
                        if callback.call::<_, ()>((stream, chunk)).is_err() {
                            let _ = ctx2.catch();
                        }
                    }
                }
                ChildEvent::Exit { code, signal } => {
                    state.children.borrow_mut().remove(&id);
                    let settled = (|| -> rquickjs::Result<()> {
                        let result = Object::new(ctx2.clone())?;
                        match code {
                            Some(code) => result.set("code", code)?,
                            None => result.set("code", Value::new_null(ctx2.clone()))?,
                        }
                        match signal {
                            Some(signal) => result.set("signal", signal)?,
                            None => result.set("signal", Value::new_null(ctx2.clone()))?,
                        }
                        result.set("stdout", stdout.as_str())?;
                        result.set("stderr", stderr.as_str())?;
                        resolve.call::<_, ()>((result,))
                    })();
                    if settled.is_err() {
                        let error = ctx2.catch();
                        let _ = reject.call::<_, ()>((error,));
                    }
                    break;
                }
            }
        }
    });
    Ok(handle)
}
