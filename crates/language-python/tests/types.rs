//! What types from ty, Ruff's type checker, add to Python answers in a
//! workspace: method calls resolved by the receiver's type, and inferred
//! types. A step turns them on by importing `types("python")`.

use language_core::{
    ByteRange, CallEdgeKind, CallGraphDirection, CallGraphOptions, SemanticProvider,
};
use language_python::RuffSemanticProvider;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct Workspace {
    _dir: TempDir,
    root: PathBuf,
    provider: RuffSemanticProvider,
}

impl Workspace {
    fn new(files: &[(&str, &str)]) -> Self {
        Self::with_types(files, true)
    }

    fn with_types(files: &[(&str, &str)], types: bool) -> Self {
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for (name, text) in files {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        let provider = RuffSemanticProvider::workspace_scope(root.clone()).with_types(types);
        Workspace {
            _dir: dir,
            root,
            provider,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// The range of the first `needle` in `file`.
    fn find(&self, file: &str, needle: &str) -> ByteRange {
        let text = fs::read_to_string(self.path(file)).unwrap();
        let start = text
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} in {file}")) as u32;
        ByteRange::new(start, start + needle.len() as u32)
    }

    fn type_of(&self, file: &str, needle: &str) -> Option<String> {
        self.provider
            .get_type(&self.path(file), self.find(file, needle))
            .unwrap()
    }

    /// Names of what the function `name` calls, with how.
    fn callees(&self, file: &str, name: &str) -> Vec<(String, CallEdgeKind)> {
        let path: &Path = &self.path(file);
        let def = self.find(file, &format!("def {name}("));
        let def = ByteRange::new(def.start + 4, def.start + 4 + name.len() as u32);
        let result = self
            .provider
            .get_call_graph(
                path,
                def,
                CallGraphOptions {
                    direction: CallGraphDirection::Callees,
                    ..Default::default()
                },
            )
            .unwrap();
        result
            .files
            .iter()
            .flat_map(|file| &file.entries)
            .filter(|entry| entry.resolved)
            .map(|entry| (entry.name.clone().unwrap_or_default(), entry.via))
            .collect()
    }
}

const REPO: &str = "class Repo:\n    def save(self, order):\n        pass\n";

const LOOP: &str = "\
from repo import Repo

def save_all(repos: list[Repo], order):
    for repo in repos:
        repo.save(order)
";

#[test]
fn a_method_called_on_a_loop_variable_resolves_by_its_element_type() {
    let ws = Workspace::new(&[("repo.py", REPO), ("main.py", LOOP)]);
    assert_eq!(
        ws.callees("main.py", "save_all"),
        vec![("save".to_string(), CallEdgeKind::Type)]
    );
}

#[test]
fn without_types_a_call_only_the_type_explains_stays_unresolved() {
    let ws = Workspace::with_types(&[("repo.py", REPO), ("main.py", LOOP)], false);
    assert_eq!(ws.callees("main.py", "save_all"), vec![]);
}

const FIRST: &str = "\
from repo import Repo

def first(repos: list[Repo]):
    head = repos[0]
    return head
";

#[test]
fn type_of_is_what_ty_infers() {
    let ws = Workspace::new(&[("repo.py", REPO), ("main.py", FIRST)]);
    assert_eq!(ws.type_of("main.py", "head").as_deref(), Some("Repo"));
}

#[test]
fn without_types_type_of_is_only_what_annotations_declare() {
    let ws = Workspace::with_types(&[("repo.py", REPO), ("main.py", FIRST)], false);
    assert_eq!(ws.type_of("main.py", "head"), None);
    assert_eq!(
        ws.type_of("main.py", "repos").as_deref(),
        Some("list[Repo]")
    );
}

const LITERALS: &str = "\
allowed = {\"a\", \"b\"}
pair = (1, 2)
";

#[test]
fn without_types_a_set_literal_is_a_set() {
    let ws = Workspace::with_types(&[("main.py", LITERALS)], false);
    assert_eq!(ws.type_of("main.py", "{").as_deref(), Some("set"));
}

#[test]
fn without_types_a_tuple_literal_is_a_tuple() {
    let ws = Workspace::with_types(&[("main.py", LITERALS)], false);
    assert_eq!(ws.type_of("main.py", "(1, 2)").as_deref(), Some("tuple"));
}

const TOOL: &str = "\
from typing import ClassVar

class Tool:
    configuration_schema: ClassVar[dict] = {}

    def schema(self):
        return self.configuration_schema
";

#[test]
fn a_class_var_read_through_self_is_its_declared_type() {
    let ws = Workspace::new(&[("tool.py", TOOL)]);
    assert_eq!(
        ws.type_of("tool.py", "self.configuration_schema")
            .as_deref(),
        Some("dict[Unknown, Unknown]")
    );
}

const PLUGIN: &str = "\
from typing import ClassVar, Optional, Type

class Config:
    pass

class Plugin:
    schema: ClassVar[Optional[Type[Config]]] = None

    def load(self):
        if self.schema is None:
            return None
        return self.schema

    @classmethod
    def describe(cls):
        return cls.schema if cls.schema else None
";

/// The type of `needle` where it first appears inside `context`.
fn type_in(ws: &Workspace, file: &str, context: &str, needle: &str) -> Option<String> {
    let around = ws.find(file, context);
    let text = fs::read_to_string(ws.path(file)).unwrap();
    let start = around.start + text[around.start as usize..].find(needle).unwrap() as u32;
    ws.provider
        .get_type(
            &ws.path(file),
            ByteRange::new(start, start + needle.len() as u32),
        )
        .unwrap()
}

#[test]
fn self_attr_read_past_a_none_check_is_the_declared_type_without_none() {
    let ws = Workspace::new(&[("plugin.py", PLUGIN)]);
    assert_eq!(
        type_in(&ws, "plugin.py", "return self.schema", "self.schema").as_deref(),
        Some("type[Config]")
    );
}

#[test]
fn cls_attr_read_where_it_is_truthy_is_the_declared_type_without_none() {
    let ws = Workspace::new(&[("plugin.py", PLUGIN)]);
    assert_eq!(
        type_in(&ws, "plugin.py", "return cls.schema", "cls.schema").as_deref(),
        Some("type[Config]")
    );
}
