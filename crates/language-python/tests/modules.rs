//! Module queries for Python: resolveModule, resolveExport, specifierFor,
//! importers and publicSpecifiers, each rule on a small temp workspace.

use language_core::{SemanticProvider, SpecifierOptions, SpecifierStyle, SymbolKind};
use language_python::RuffSemanticProvider;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// A temp workspace and a workspace-scope provider over it.
struct Ws {
    dir: TempDir,
    provider: RuffSemanticProvider,
}

impl Ws {
    fn new(files: &[(&str, &str)]) -> Ws {
        let dir = TempDir::new().unwrap();
        for (path, text) in files {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let provider = RuffSemanticProvider::workspace_scope(dir.path().to_path_buf());
        Ws { dir, provider }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().canonicalize().unwrap().join(rel)
    }

    fn rel(&self, path: &Path) -> String {
        let root = self.dir.path().canonicalize().unwrap();
        path.strip_prefix(root).unwrap().to_string_lossy().into()
    }

    fn resolve(&self, from: &str, specifier: &str) -> Option<String> {
        let found = self
            .provider
            .resolve_module(&self.path(from), specifier)
            .unwrap();
        found.map(|p| self.rel(&p))
    }

    /// Where `name` lands, as `file#name`; the range holds the name (a
    /// module landing spans its file).
    fn export(&self, module: &str, name: &str) -> Option<String> {
        let found = self
            .provider
            .resolve_export(&self.path(module), name)
            .unwrap()?;
        let location = &found.location;
        let text = &found.content[location.range.start as usize..location.range.end as usize];
        match location.kind {
            SymbolKind::Namespace => assert_eq!(text, found.content),
            _ => assert_eq!(text, location.name),
        }
        Some(format!(
            "{}#{}",
            self.rel(&location.file_path),
            location.name
        ))
    }

    fn specifier(&self, from: &str, target: &str, options: SpecifierOptions) -> Option<String> {
        self.provider
            .specifier_for(&self.path(from), &self.path(target), &options)
            .unwrap()
    }

    /// `file kind: text` for each importer, the text being its range.
    fn importers(&self, target: &str) -> Vec<String> {
        let found = self.provider.importers(&self.path(target)).unwrap();
        found
            .files
            .iter()
            .flat_map(|file| {
                file.importers.iter().map(|i| {
                    let text = &file.content[i.range.start as usize..i.range.end as usize];
                    format!(
                        "{} {}: {}",
                        self.rel(&file.file_path),
                        i.kind.as_str(),
                        text
                    )
                })
            })
            .collect()
    }

    fn public(&self, file: &str) -> Vec<String> {
        self.provider.public_specifiers(&self.path(file)).unwrap()
    }
}

fn prefer(style: SpecifierStyle) -> SpecifierOptions {
    SpecifierOptions {
        prefer: Some(style),
        like: None,
    }
}

fn like(specifier: &str) -> SpecifierOptions {
    SpecifierOptions {
        prefer: None,
        like: Some(specifier.to_string()),
    }
}

const SHOP: &[(&str, &str)] = &[
    ("shop/__init__.py", ""),
    ("shop/core/__init__.py", ""),
    ("shop/core/db.py", "def connect(): ...\n"),
    ("shop/orders/__init__.py", ""),
    ("shop/orders/models.py", "class Order: ...\n"),
    ("shop/orders/views.py", "from .models import Order\n"),
];

// --- resolveModule --------------------------------------------------------

#[test]
fn an_absolute_dotted_import_resolves_from_the_workspace_root() {
    let ws = Ws::new(SHOP);
    assert_eq!(
        ws.resolve("shop/orders/views.py", "shop.core.db")
            .as_deref(),
        Some("shop/core/db.py")
    );
}

#[test]
fn an_absolute_import_resolves_under_a_src_layout() {
    let ws = Ws::new(&[
        ("src/app/__init__.py", ""),
        ("src/app/db.py", ""),
        ("tests/test_db.py", ""),
    ]);
    assert_eq!(
        ws.resolve("tests/test_db.py", "app.db").as_deref(),
        Some("src/app/db.py")
    );
}

#[test]
fn a_relative_import_resolves_against_the_importing_package() {
    let ws = Ws::new(SHOP);
    assert_eq!(
        ws.resolve("shop/orders/views.py", ".models").as_deref(),
        Some("shop/orders/models.py")
    );
}

#[test]
fn each_extra_leading_dot_climbs_one_package() {
    let ws = Ws::new(SHOP);
    assert_eq!(
        ws.resolve("shop/orders/views.py", "..core.db").as_deref(),
        Some("shop/core/db.py")
    );
}

#[test]
fn a_bare_dot_resolves_to_the_package_init() {
    let ws = Ws::new(SHOP);
    assert_eq!(
        ws.resolve("shop/orders/views.py", ".").as_deref(),
        Some("shop/orders/__init__.py")
    );
}

#[test]
fn a_stub_resolves_when_there_is_no_source_file() {
    let ws = Ws::new(&[("typed.pyi", ""), ("main.py", "")]);
    assert_eq!(ws.resolve("main.py", "typed").as_deref(), Some("typed.pyi"));
}

#[test]
fn an_installed_package_resolves_from_the_workspace_virtualenv() {
    let ws = Ws::new(&[
        (
            ".venv/lib/python3.12/site-packages/requests/__init__.py",
            "",
        ),
        ("main.py", ""),
    ]);
    assert_eq!(
        ws.resolve("main.py", "requests").as_deref(),
        Some(".venv/lib/python3.12/site-packages/requests/__init__.py")
    );
}

#[test]
fn an_unknown_module_resolves_to_none() {
    let ws = Ws::new(SHOP);
    assert_eq!(ws.resolve("shop/orders/views.py", "shop.missing"), None);
}

#[test]
fn module_queries_need_workspace_scope() {
    let ws = Ws::new(SHOP);
    let file_scope = RuffSemanticProvider::file_scope();
    let error = file_scope
        .resolve_module(&ws.path("shop/orders/views.py"), ".models")
        .unwrap_err();
    assert!(error.to_string().contains("workspace"), "{error}");
}

// --- resolveExport --------------------------------------------------------

#[test]
fn a_defined_name_lands_on_its_definition() {
    let ws = Ws::new(SHOP);
    assert_eq!(
        ws.export("shop/orders/models.py", "Order").as_deref(),
        Some("shop/orders/models.py#Order")
    );
}

#[test]
fn a_reexport_through_init_files_lands_where_the_name_is_defined() {
    let ws = Ws::new(&[
        ("shop/__init__.py", "from shop.orders import Order\n"),
        ("shop/orders/__init__.py", "from .models import Order\n"),
        ("shop/orders/models.py", "class Order: ...\n"),
    ]);
    assert_eq!(
        ws.export("shop/__init__.py", "Order").as_deref(),
        Some("shop/orders/models.py#Order")
    );
}

#[test]
fn an_aliased_reexport_follows_the_original_name() {
    let ws = Ws::new(&[
        ("api.py", "from impl import make as build\n"),
        ("impl.py", "def make(): ...\n"),
    ]);
    assert_eq!(
        ws.export("api.py", "build").as_deref(),
        Some("impl.py#make")
    );
}

#[test]
fn a_submodule_name_lands_on_the_module_file() {
    let ws = Ws::new(SHOP);
    assert_eq!(
        ws.export("shop/orders/__init__.py", "models").as_deref(),
        Some("shop/orders/models.py#shop.orders.models")
    );
}

#[test]
fn a_star_import_reexports_the_public_names_it_brings_in() {
    let ws = Ws::new(&[
        ("api.py", "from impl import *\n"),
        ("impl.py", "def make(): ...\ndef _hidden(): ...\n"),
    ]);
    assert_eq!(
        (ws.export("api.py", "make"), ws.export("api.py", "_hidden")),
        (Some("impl.py#make".to_string()), None)
    );
}

#[test]
fn a_reexport_from_an_unresolved_module_stops_at_the_import() {
    let ws = Ws::new(&[("api.py", "from elsewhere import thing\n")]);
    assert_eq!(
        ws.export("api.py", "thing").as_deref(),
        Some("api.py#thing")
    );
}

#[test]
fn an_absent_name_resolves_to_none() {
    let ws = Ws::new(SHOP);
    assert_eq!(ws.export("shop/orders/models.py", "Missing"), None);
}

// --- specifierFor ---------------------------------------------------------

#[test]
fn a_file_in_an_importable_package_gets_an_absolute_specifier_by_default() {
    let ws = Ws::new(SHOP);
    assert_eq!(
        ws.specifier(
            "shop/orders/views.py",
            "shop/core/db.py",
            Default::default()
        )
        .as_deref(),
        Some("shop.core.db")
    );
}

#[test]
fn prefer_relative_climbs_to_the_common_package() {
    let ws = Ws::new(SHOP);
    assert_eq!(
        ws.specifier(
            "shop/orders/views.py",
            "shop/core/db.py",
            prefer(SpecifierStyle::Relative)
        )
        .as_deref(),
        Some("..core.db")
    );
}

#[test]
fn like_keeps_the_style_of_the_replaced_specifier() {
    let ws = Ws::new(SHOP);
    let from = "shop/orders/views.py";
    assert_eq!(
        (
            ws.specifier(from, "shop/orders/models.py", like(".models")),
            ws.specifier(from, "shop/orders/models.py", like("shop.orders.models")),
        ),
        (
            Some(".models".to_string()),
            Some("shop.orders.models".to_string())
        )
    );
}

#[test]
fn a_package_is_named_without_its_init() {
    let ws = Ws::new(SHOP);
    assert_eq!(
        ws.specifier(
            "shop/orders/views.py",
            "shop/core/__init__.py",
            Default::default()
        )
        .as_deref(),
        Some("shop.core")
    );
}

#[test]
fn a_file_outside_the_search_roots_gets_a_relative_specifier_by_default() {
    let ws = Ws::new(&[
        ("my-tools/kit/__init__.py", ""),
        ("my-tools/kit/a.py", ""),
        ("my-tools/kit/b.py", ""),
    ]);
    assert_eq!(
        ws.specifier("my-tools/kit/a.py", "my-tools/kit/b.py", Default::default())
            .as_deref(),
        Some(".b")
    );
}

#[test]
fn a_relative_specifier_never_climbs_above_the_top_level_package() {
    let ws = Ws::new(&[
        ("shop/__init__.py", ""),
        ("shop/a.py", ""),
        ("other/__init__.py", ""),
        ("other/b.py", ""),
    ]);
    assert_eq!(
        ws.specifier("shop/a.py", "other/b.py", prefer(SpecifierStyle::Relative))
            .as_deref(),
        Some("other.b")
    );
}

// --- importers ------------------------------------------------------------

#[test]
fn import_statements_naming_the_module_are_importers() {
    let ws = Ws::new(&[
        ("shop/__init__.py", ""),
        ("shop/models.py", ""),
        ("a.py", "import shop.models\nimport shop.models as m\n"),
        ("b.py", "from shop.models import Order\n"),
        ("shop/c.py", "from .models import Order\n"),
    ]);
    assert_eq!(
        ws.importers("shop/models.py"),
        [
            "a.py import: import shop.models",
            "a.py import: import shop.models as m",
            "b.py import: from shop.models import Order",
            "shop/c.py import: from .models import Order",
        ]
    );
}

#[test]
fn importing_a_submodule_by_name_from_its_package_is_an_importer() {
    let ws = Ws::new(&[
        ("shop/__init__.py", ""),
        ("shop/models.py", ""),
        ("a.py", "from shop import models\n"),
        ("shop/c.py", "from . import models\n"),
    ]);
    assert_eq!(
        ws.importers("shop/models.py"),
        [
            "a.py import: from shop import models",
            "shop/c.py import: from . import models",
        ]
    );
}

#[test]
fn literal_import_module_and_dunder_import_calls_are_dynamic_importers() {
    let ws = Ws::new(&[
        ("shop/__init__.py", ""),
        ("shop/models.py", ""),
        (
            "a.py",
            "import importlib\nimportlib.import_module('shop.models')\n__import__('shop.models')\nimportlib.import_module('.models', package='shop')\n",
        ),
    ]);
    assert_eq!(
        ws.importers("shop/models.py"),
        [
            "a.py dynamic: importlib.import_module('shop.models')",
            "a.py dynamic: __import__('shop.models')",
            "a.py dynamic: importlib.import_module('.models', package='shop')",
        ]
    );
}

#[test]
fn patch_and_monkeypatch_targets_are_mock_importers() {
    let ws = Ws::new(&[
        ("shop/__init__.py", ""),
        ("shop/models.py", "def save(): ...\n"),
        (
            "test_a.py",
            "from unittest.mock import patch\n\n@patch('shop.models.save')\ndef test_a(monkeypatch, mocker):\n    mocker.patch('shop.models.save')\n    monkeypatch.setattr('shop.models.save', None)\n",
        ),
    ]);
    assert_eq!(
        ws.importers("shop/models.py"),
        [
            "test_a.py mock: patch('shop.models.save')",
            "test_a.py mock: mocker.patch('shop.models.save')",
            "test_a.py mock: monkeypatch.setattr('shop.models.save', None)",
        ]
    );
}

#[test]
fn a_mock_target_refers_only_to_its_longest_module_prefix() {
    let ws = Ws::new(&[
        ("shop/__init__.py", ""),
        ("shop/models.py", "class Order: ...\n"),
        (
            "test_a.py",
            "from unittest import mock\nmock.patch('shop.models.Order.save')\n",
        ),
    ]);
    assert_eq!(
        (
            ws.importers("shop/models.py").len(),
            ws.importers("shop/__init__.py")
        ),
        (1, Vec::<String>::new())
    );
}

#[test]
fn an_import_of_a_same_named_module_elsewhere_is_not_an_importer() {
    let ws = Ws::new(&[
        ("shop/__init__.py", ""),
        ("shop/models.py", ""),
        ("blog/__init__.py", ""),
        ("blog/models.py", ""),
        (
            "blog/views.py",
            "from .models import Post\nfrom blog.models import Tag\n",
        ),
    ]);
    assert_eq!(ws.importers("shop/models.py"), Vec::<String>::new());
}

#[test]
fn files_that_do_not_parse_are_reported_as_unparsed() {
    let ws = Ws::new(&[("shop.py", ""), ("broken.py", "def (:\n")]);
    let found = ws.provider.importers(&ws.path("shop.py")).unwrap();
    let unparsed: Vec<String> = found
        .unparsed
        .iter()
        .map(|u| ws.rel(&u.file_path))
        .collect();
    assert_eq!(unparsed, ["broken.py"]);
}

#[test]
fn importers_read_files_as_pinned_when_the_step_started() {
    let ws = Ws::new(&[("shop.py", ""), ("a.py", "")]);
    ws.provider
        .notify_file_processed(&ws.path("a.py"), "import shop\n")
        .unwrap();
    assert_eq!(ws.importers("shop.py"), ["a.py import: import shop"]);
}

// --- publicSpecifiers -----------------------------------------------------

#[test]
fn a_module_is_public_under_its_dotted_name() {
    let ws = Ws::new(SHOP);
    assert_eq!(ws.public("shop/orders/views.py"), ["shop.orders.views"]);
}

#[test]
fn a_package_init_is_public_under_the_package_name() {
    let ws = Ws::new(SHOP);
    assert_eq!(ws.public("shop/orders/__init__.py"), ["shop.orders"]);
}

#[test]
fn a_src_layout_module_is_named_from_src() {
    let ws = Ws::new(&[("src/app/__init__.py", ""), ("src/app/db.py", "")]);
    assert_eq!(ws.public("src/app/db.py"), ["app.db"]);
}

#[test]
fn a_file_with_no_importable_name_has_no_public_specifiers() {
    let ws = Ws::new(&[("my-tools/run.py", "")]);
    assert_eq!(ws.public("my-tools/run.py"), Vec::<String>::new());
}
