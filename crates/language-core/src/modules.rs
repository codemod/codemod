//! Module resolution exposed to codemods: what a specifier resolves to, what
//! a module's export lands on, which specifier reaches a file, and which
//! files import one.

use crate::{ByteRange, UnparsedFile};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Which kind of specifier `specifier_for` should prefer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecifierStyle {
    /// `./a/b`, `../c`.
    Relative,
    /// A tsconfig `paths` alias (`@/components/button`, `~utils`).
    Alias,
}

/// Options for `specifier_for`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpecifierOptions {
    /// Prefer this style when both kinds reach the file.
    pub prefer: Option<SpecifierStyle>,
    /// The specifier being replaced. Its style (relative or alias), its alias
    /// prefix and whether it names an extension (`./x.js`) are kept when a
    /// specifier like it reaches the file. Takes precedence over `prefer`.
    pub like: Option<String>,
}

/// How a file refers to a module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImportKind {
    /// `import … from "m"`, `import "m"`.
    Import,
    /// `export … from "m"`, `export * from "m"`.
    Export,
    /// `require("m")`.
    Require,
    /// `import("m")`.
    Dynamic,
    /// A test-framework module reference: `jest.mock("m")`, `vi.mock`,
    /// `jest.requireActual`, `vi.importActual`, and their siblings.
    Mock,
}

impl ImportKind {
    /// The name JavaScript callers see.
    pub fn as_str(&self) -> &'static str {
        match self {
            ImportKind::Import => "import",
            ImportKind::Export => "export",
            ImportKind::Require => "require",
            ImportKind::Dynamic => "dynamic",
            ImportKind::Mock => "mock",
        }
    }
}

/// One place that refers to the queried module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Importer {
    /// The statement (`import`, `export … from`) or call (`require`,
    /// `import()`, `jest.mock`) that names the module.
    pub range: ByteRange,
    pub kind: ImportKind,
    /// The specifier as written.
    pub specifier: String,
}

/// Importers in one file, with its content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportersFile {
    pub file_path: PathBuf,
    pub content: String,
    pub importers: Vec<Importer>,
}

/// Result of an importers query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportersResult {
    pub files: Vec<ImportersFile>,
    /// Workspace files that did not parse: references in them are missing.
    pub unparsed: Vec<UnparsedFile>,
}
