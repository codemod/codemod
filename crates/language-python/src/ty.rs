//! Types from ty, Ruff's Python type checker, run in process.
//!
//! One [`TySession`] per provider (so per step): a ty project database over
//! the workspace, built on the first typed question. It reads files through
//! the provider's [`FileSnapshot`], so its answers describe the workspace as
//! the step started, like every other answer, and it sees the workspace's
//! virtual environment (`.venv`, `VIRTUAL_ENV`) the way `ty check` does, so
//! `django`, `fastapi` and `pydantic` types resolve when they are installed.
//!
//! Two questions use it: the type of an expression or binding
//! ([`TySession::type_at`]), and where the attribute a member call names is
//! declared ([`TySession::member_declarations`]), which resolves
//! `repo.save()` when `repo` is a parameter, a field or a call's result.

use language_core::{ByteRange, FileSnapshot};
use parking_lot::Mutex;
use ruff_db::files::{system_path_to_file, File};
use ruff_db::parsed::parsed_module;
use ruff_db::system::walk_directory::WalkDirectoryBuilder;
use ruff_db::system::{
    CaseSensitivity, DirectoryEntry, GlobError, Metadata, OsSystem, PatternError, System,
    SystemPath, SystemPathBuf, SystemVirtualPath, WritableSystem,
};
use ruff_notebook::{Notebook, NotebookError};
use ruff_python_ast::visitor::source_order::{self, SourceOrderVisitor, TraversalSignal};
use ruff_python_ast::{self as ast, AnyNodeRef};
use ruff_text_size::{Ranged, TextRange, TextSize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use ty_project::{ProjectDatabase, ProjectMetadata};
use ty_python_semantic::{HasType, SemanticModel};

/// The real file system, except that file texts come from the step's
/// snapshot: a file reads the same however often ty reads it, and a write
/// the step makes is not seen until the next step.
#[derive(Debug, Clone)]
struct SnapshotSystem {
    os: OsSystem,
    snapshot: Arc<FileSnapshot>,
}

impl System for SnapshotSystem {
    fn path_metadata(&self, path: &SystemPath) -> std::io::Result<Metadata> {
        self.os.path_metadata(path)
    }

    fn canonicalize_path(&self, path: &SystemPath) -> std::io::Result<SystemPathBuf> {
        self.os.canonicalize_path(path)
    }

    fn read_to_string(&self, path: &SystemPath) -> std::io::Result<String> {
        self.snapshot
            .get_or_read(path.as_std_path(), || self.os.read_to_string(path))
            .map(|text| text.to_string())
    }

    fn read_to_notebook(&self, path: &SystemPath) -> Result<Notebook, NotebookError> {
        self.os.read_to_notebook(path)
    }

    fn read_virtual_path_to_string(&self, path: &SystemVirtualPath) -> std::io::Result<String> {
        self.os.read_virtual_path_to_string(path)
    }

    fn read_virtual_path_to_notebook(
        &self,
        path: &SystemVirtualPath,
    ) -> Result<Notebook, NotebookError> {
        self.os.read_virtual_path_to_notebook(path)
    }

    fn path_exists_case_sensitive(&self, path: &SystemPath, prefix: &SystemPath) -> bool {
        self.os.path_exists_case_sensitive(path, prefix)
    }

    fn case_sensitivity(&self) -> CaseSensitivity {
        self.os.case_sensitivity()
    }

    fn current_directory(&self) -> &SystemPath {
        self.os.current_directory()
    }

    /// None: a user's own ty settings must not change a codemod's answers.
    fn user_config_directory(&self) -> Option<SystemPathBuf> {
        None
    }

    fn cache_dir(&self) -> Option<SystemPathBuf> {
        self.os.cache_dir()
    }

    fn read_directory<'a>(
        &'a self,
        path: &SystemPath,
    ) -> std::io::Result<Box<dyn Iterator<Item = std::io::Result<DirectoryEntry>> + 'a>> {
        self.os.read_directory(path)
    }

    fn walk_directory(&self, path: &SystemPath) -> WalkDirectoryBuilder {
        self.os.walk_directory(path)
    }

    fn glob(
        &self,
        pattern: &str,
    ) -> Result<Box<dyn Iterator<Item = Result<SystemPathBuf, GlobError>> + '_>, PatternError> {
        self.os.glob(pattern)
    }

    fn env_var(&self, name: &str) -> Result<String, std::env::VarError> {
        self.os.env_var(name)
    }

    fn as_writable(&self) -> Option<&dyn WritableSystem> {
        None
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn dyn_clone(&self) -> Box<dyn System> {
        Box::new(self.clone())
    }
}

/// ty over one workspace, for one step.
pub(crate) struct TySession {
    root: PathBuf,
    snapshot: Arc<FileSnapshot>,
    /// Built on first use; None when ty could not set the project up.
    db: OnceLock<Option<Mutex<ProjectDatabase>>>,
}

impl std::fmt::Debug for TySession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TySession")
            .field("root", &self.root)
            .field("built", &self.db.get().is_some())
            .finish()
    }
}

impl TySession {
    pub(crate) fn new(root: PathBuf, snapshot: Arc<FileSnapshot>) -> Self {
        TySession {
            root,
            snapshot,
            db: OnceLock::new(),
        }
    }

    /// A handle on the database: clones share its caches, and each thread
    /// asks through its own.
    fn db(&self) -> Option<ProjectDatabase> {
        self.db
            .get_or_init(|| match self.build() {
                Ok(db) => Some(Mutex::new(db)),
                Err(error) => {
                    log::warn!(
                        "Python types: ty could not load {}: {error}",
                        self.root.display()
                    );
                    None
                }
            })
            .as_ref()
            .map(|db| db.lock().clone())
    }

    fn build(&self) -> anyhow::Result<ProjectDatabase> {
        let root = SystemPathBuf::from_path_buf(self.root.clone())
            .map_err(|path| anyhow::anyhow!("not a UTF-8 path: {}", path.display()))?;
        let system = SnapshotSystem {
            os: OsSystem::new(&root),
            snapshot: Arc::clone(&self.snapshot),
        };
        // The workspace's own pyproject.toml / ty.toml (Python version,
        // source roots); without one, defaults rooted at the workspace.
        let metadata = ProjectMetadata::discover(&root, &system)
            .ok()
            .filter(|found| found.root().starts_with(&root))
            .unwrap_or_else(|| ProjectMetadata::new("workspace".into(), root.clone()));
        ProjectDatabase::new(metadata, system)
    }

    fn file(db: &ProjectDatabase, path: &Path) -> Option<File> {
        let path = SystemPath::from_std_path(path)?;
        system_path_to_file(db, path).ok()
    }

    /// The type ty infers for the expression or binding at `range`, as ty
    /// prints it (`Order`, `list[str]`, `str | None`,
    /// `def total(self) -> Decimal`). None when ty knows nothing about it.
    pub(crate) fn type_at(&self, path: &Path, range: ByteRange) -> Option<String> {
        let db = self.db()?;
        let file = Self::file(&db, path)?;
        let parsed = parsed_module(&db, file).load(&db);
        let target = Target::find(parsed.syntax(), range)?;
        let model = SemanticModel::new(&db, file);
        let attribute = match &target {
            Target::Expr(ast::ExprRef::Attribute(attribute)) => Some(*attribute),
            _ => None,
        };
        let ty = match target {
            Target::Expr(expr) => expr.inferred_type(&model),
            Target::Function(function) => function.inferred_type(&model),
            Target::Class(class) => class.inferred_type(&model),
            Target::Parameter(parameter) => parameter.inferred_type(&model),
            Target::Alias(alias) => alias.inferred_type(&model),
        };
        let printed = ty.display(&db).to_string();
        if known(&printed) {
            return Some(printed);
        }
        // `self.x` / `cls.x`: this ty types an unannotated `self` as Unknown
        // (narrowed: `Unknown & ~None`). The class attribute's declared type,
        // narrowed the same way, is what the read holds.
        let declared = class_attribute(parsed.syntax(), attribute?)?;
        let declared = ast::ExprRef::from(declared).inferred_type(&model);
        narrowed(&declared.display(&db).to_string(), &printed)
    }

    /// Where ty says the attribute starting at `offset` (the `save` of
    /// `repo.save()`) is declared: each file with the range of the declared
    /// name. Empty when ty cannot tell.
    pub(crate) fn member_declarations(
        &self,
        path: &Path,
        offset: u32,
    ) -> Vec<(PathBuf, ByteRange)> {
        let Some(db) = self.db() else {
            return Vec::new();
        };
        let Some(file) = Self::file(&db, path) else {
            return Vec::new();
        };
        let Some(found) = ty_ide::goto_definition(&db, file, TextSize::from(offset)) else {
            return Vec::new();
        };
        found
            .value
            .into_iter()
            .filter_map(|target| {
                let path = target.file().path(&db).as_system_path()?.as_std_path();
                let range = target.focus_range();
                Some((
                    path.to_path_buf(),
                    ByteRange::new(range.start().to_u32(), range.end().to_u32()),
                ))
            })
            .collect()
    }
}

/// Whether ty's printed type says something: some arm of it is more than
/// `Unknown` (`Unknown & ~None`, what narrowing an unknown leaves, is not),
/// and it holds none of ty's placeholders for what it does not model yet
/// (`@Todo`). `Unknown | ForeignKey` is known, and kept whole: the Unknown
/// arm says the attribute's declared type is not known.
fn known(printed: &str) -> bool {
    !printed.is_empty()
        && !printed.contains("@Todo")
        && split_top(printed, " | ").iter().any(|arm| {
            split_top(unparenthesized(arm), " & ")
                .iter()
                .any(|part| !part.starts_with('~') && *part != "Unknown")
        })
}

/// `printed` split at `separator` outside brackets and parentheses.
fn split_top<'a>(printed: &'a str, separator: &str) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let bytes = printed.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'[' | b'(' => depth += 1,
            b']' | b')' => depth -= 1,
            _ if depth == 0 && printed[at..].starts_with(separator) => {
                parts.push(&printed[start..at]);
                at += separator.len();
                start = at;
                continue;
            }
            _ => {}
        }
        at += 1;
    }
    parts.push(&printed[start..]);
    parts
}

/// `(a & b)` without its parentheses.
fn unparenthesized(arm: &str) -> &str {
    arm.strip_prefix('(')
        .and_then(|inner| inner.strip_suffix(')'))
        .filter(|inner| split_top(inner, ")").len() == 1)
        .unwrap_or(arm)
}

/// `declared` with what narrowing took away from the read (`~None`,
/// `~AlwaysFalsy` in `narrowed_read`): `type[Config] | None` read past an
/// `is None` check is `type[Config]`. None when the declared type is not
/// known, or the read was narrowed in a way this does not follow.
fn narrowed(declared: &str, narrowed_read: &str) -> Option<String> {
    if !known(declared) {
        return None;
    }
    let mut arms = split_top(declared, " | ");
    let read_arms = split_top(narrowed_read, " | ");
    let [read] = read_arms.as_slice() else {
        return None;
    };
    for part in split_top(unparenthesized(read), " & ") {
        match part {
            "Unknown" => {}
            // None is always falsy: both leave it out.
            "~None" | "~AlwaysFalsy" => arms.retain(|arm| *arm != "None"),
            _ => return None,
        }
    }
    (!arms.is_empty()).then(|| arms.join(" | "))
}

/// What says the type of the class attribute `attribute` reads, when it reads
/// it through the first parameter (`self`, `cls`) of a method of that class:
/// the annotation it is declared with (`ClassVar[Optional[Type[Config]]]` of
/// `schema: ClassVar[Optional[Type[Config]]] = None`, which ty types as
/// `type[Config] | None`), else the name a plain assignment binds.
fn class_attribute<'a>(
    module: &'a ast::ModModule,
    attribute: &ast::ExprAttribute,
) -> Option<&'a ast::Expr> {
    let ast::Expr::Name(object) = attribute.value.as_ref() else {
        return None;
    };
    let mut finder = MethodFinder {
        range: attribute.range,
        classes: Vec::new(),
        found: None,
    };
    source_order::walk_body(&mut finder, &module.body);
    let (class, method) = finder.found?;
    let first = method
        .parameters
        .posonlyargs
        .iter()
        .chain(method.parameters.args.iter())
        .next()?;
    if first.parameter.name.as_str() != object.id.as_str() {
        return None;
    }
    let name = attribute.attr.as_str();
    let is_name =
        |target: &ast::Expr| matches!(target, ast::Expr::Name(n) if n.id.as_str() == name);
    let declared = class.body.iter().find_map(|stmt| match stmt {
        ast::Stmt::AnnAssign(assign) if is_name(&assign.target) => Some(&*assign.annotation),
        _ => None,
    });
    declared.or_else(|| {
        class.body.iter().find_map(|stmt| match stmt {
            ast::Stmt::Assign(assign) => assign.targets.iter().find(|t| is_name(t)),
            _ => None,
        })
    })
}

/// The innermost method around a range, with its class.
struct MethodFinder<'a> {
    range: TextRange,
    classes: Vec<&'a ast::StmtClassDef>,
    found: Option<(&'a ast::StmtClassDef, &'a ast::StmtFunctionDef)>,
}

impl<'a> SourceOrderVisitor<'a> for MethodFinder<'a> {
    fn enter_node(&mut self, node: AnyNodeRef<'a>) -> TraversalSignal {
        if !node.range().contains_range(self.range) {
            return TraversalSignal::Skip;
        }
        match node {
            AnyNodeRef::StmtClassDef(class) => self.classes.push(class),
            AnyNodeRef::StmtFunctionDef(function) => {
                if let Some(class) = self.classes.last() {
                    let in_body = class.body.iter().any(
                        |stmt| matches!(stmt, ast::Stmt::FunctionDef(f) if std::ptr::eq(f, function)),
                    );
                    if in_body {
                        self.found = Some((class, function));
                    }
                }
            }
            _ => {}
        }
        TraversalSignal::Traverse
    }

    fn leave_node(&mut self, node: AnyNodeRef<'a>) {
        if let AnyNodeRef::StmtClassDef(class) = node {
            if self
                .classes
                .last()
                .is_some_and(|last| std::ptr::eq(*last, class))
            {
                self.classes.pop();
            }
        }
    }
}

/// What a range names, for the type question.
enum Target<'a> {
    Expr(ast::ExprRef<'a>),
    Function(&'a ast::StmtFunctionDef),
    Class(&'a ast::StmtClassDef),
    Parameter(&'a ast::Parameter),
    Alias(&'a ast::Alias),
}

impl<'a> Target<'a> {
    /// The node `range` is: an expression with exactly that range, or the
    /// definition whose name (or whole statement) it is. Else the smallest
    /// expression around it.
    fn find(module: &'a ast::ModModule, range: ByteRange) -> Option<Self> {
        let range = TextRange::new(TextSize::from(range.start), TextSize::from(range.end));
        let mut finder = Finder {
            range,
            exact: None,
            covering: None,
        };
        source_order::walk_body(&mut finder, &module.body);
        finder.exact.or(finder.covering)
    }
}

struct Finder<'a> {
    range: TextRange,
    exact: Option<Target<'a>>,
    covering: Option<Target<'a>>,
}

impl<'a> SourceOrderVisitor<'a> for Finder<'a> {
    fn enter_node(&mut self, node: AnyNodeRef<'a>) -> TraversalSignal {
        if !node.range().contains_range(self.range) {
            return TraversalSignal::Skip;
        }
        let named = |name: TextRange| name == self.range;
        let exact = node.range() == self.range;
        let target = match node {
            AnyNodeRef::StmtFunctionDef(function) if exact || named(function.name.range) => {
                Some(Target::Function(function))
            }
            AnyNodeRef::StmtClassDef(class) if exact || named(class.name.range) => {
                Some(Target::Class(class))
            }
            AnyNodeRef::Parameter(parameter) if exact || named(parameter.name.range) => {
                Some(Target::Parameter(parameter))
            }
            AnyNodeRef::Alias(alias)
                if exact
                    || named(alias.name.range)
                    || alias.asname.as_ref().is_some_and(|n| named(n.range)) =>
            {
                Some(Target::Alias(alias))
            }
            _ => None,
        };
        if let Some(target) = target {
            self.exact = Some(target);
            return TraversalSignal::Skip;
        }
        if let Some(expr) = node.as_expr_ref() {
            // Visited outside in: the innermost expression with the range
            // wins, as does the innermost one around it.
            if exact {
                self.exact = Some(Target::Expr(expr));
            } else {
                self.covering = Some(Target::Expr(expr));
            }
        }
        TraversalSignal::Traverse
    }
}

#[cfg(test)]
mod tests {
    use super::{known, narrowed};

    #[test]
    fn what_narrowing_an_unknown_leaves_is_not_known() {
        assert!(!known("Unknown"));
        assert!(!known("Unknown & ~None"));
        assert!(!known("Unknown & ~AlwaysFalsy"));
        assert!(!known("(Unknown & ~None) | Unknown"));
        assert!(known("Unknown | ForeignKey"));
        assert!(known("Order & ~None"));
        assert!(!known("@Todo(generics)"));
    }

    #[test]
    fn a_declared_type_narrowed_like_the_read_drops_none() {
        assert_eq!(
            narrowed("type[Config] | None", "Unknown & ~None").as_deref(),
            Some("type[Config]")
        );
        assert_eq!(
            narrowed("type[Config] | None", "Unknown & ~AlwaysFalsy").as_deref(),
            Some("type[Config]")
        );
        assert_eq!(narrowed("type[Config] | None", "Unknown & ~int"), None);
        assert_eq!(narrowed("None", "Unknown & ~None"), None);
        assert_eq!(narrowed("Unknown", "Unknown"), None);
    }
}
