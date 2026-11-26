//! Where `symbols("python").definition` lands for a use of an imported name,
//! when the run told the provider only about the importing file.

use language_core::{ByteRange, DefinitionOptions, SemanticProvider};
use language_python::RuffSemanticProvider;
use tempfile::TempDir;

/// Where the last use of `name` in `file` lands, as `file:line kind name`,
/// on a workspace provider told only about `file`.
fn definition(files: &[(&str, &str)], file: &str, name: &str) -> Option<String> {
    let dir = TempDir::new().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for (path, text) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let provider = RuffSemanticProvider::workspace_scope(root.clone());
    let path = root.join(file);
    let text = std::fs::read_to_string(&path).unwrap();
    provider.notify_file_processed(&path, &text).unwrap();
    let start = text.rfind(name).unwrap() as u32;
    let range = ByteRange::new(start, start + name.len() as u32);
    let found = provider
        .get_definition(&path, range, DefinitionOptions::default())
        .unwrap()?;
    let at = &found.location;
    let line = found.content[..at.range.start as usize]
        .matches('\n')
        .count()
        + 1;
    Some(format!(
        "{}:{line} {} {}",
        at.file_path.strip_prefix(&root).unwrap().display(),
        found.kind.as_str(),
        &found.content[at.range.start as usize..at.range.end as usize],
    ))
}

#[test]
fn a_from_import_lands_on_the_declaration_in_the_imported_module() {
    let files = [
        ("pkg/__init__.py", ""),
        (
            "pkg/signals.py",
            "from x import Signal\norder_placed = Signal()\n",
        ),
        (
            "pkg/checkout.py",
            "from pkg.signals import order_placed\n\ndef place():\n    order_placed.send()\n",
        ),
    ];
    assert_eq!(
        definition(&files, "pkg/checkout.py", "order_placed").as_deref(),
        Some("pkg/signals.py:2 external order_placed")
    );
}

#[test]
fn a_name_a_package_init_re_exports_lands_where_it_is_declared() {
    let files = [
        ("app/__init__.py", "from .pagination import paginate\n"),
        (
            "app/pagination.py",
            "def paginate(rows):\n    return rows\n",
        ),
        ("main.py", "from app import paginate\n\npaginate([])\n"),
    ];
    assert_eq!(
        definition(&files, "main.py", "paginate").as_deref(),
        Some("app/pagination.py:1 external paginate")
    );
}

#[test]
fn an_import_from_a_module_outside_the_workspace_stays_the_import() {
    let files = [(
        "main.py",
        "from django.dispatch import receiver\n\nreceiver(1)\n",
    )];
    assert_eq!(
        definition(&files, "main.py", "receiver").as_deref(),
        Some("main.py:1 import receiver")
    );
}
