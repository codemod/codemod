use ast_grep_core::tree_sitter::{LanguageExt, TSLanguage};
use ast_grep_core::Language;
use ast_grep_core::matcher::{Pattern, PatternBuilder, PatternError};
use ast_grep_dynamic::DynamicLang;
use ast_grep_language::SupportLang;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;

/// A language type that wraps both statically-linked `SupportLang` (from ast-grep)
/// and dynamically-loaded `DynamicLang` (from tree-sitter-loader).
///
/// This allows the engine to support languages beyond the 26 built into ast-grep
/// by downloading and loading tree-sitter parsers at runtime.
#[derive(Clone, Copy)]
pub enum CodemodLang {
    Static(SupportLang),
    Dynamic(DynamicLang),
}

impl PartialEq for CodemodLang {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (CodemodLang::Static(a), CodemodLang::Static(b)) => a == b,
            (CodemodLang::Dynamic(a), CodemodLang::Dynamic(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for CodemodLang {}

impl Hash for CodemodLang {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            CodemodLang::Static(lang) => {
                0u8.hash(state);
                lang.hash(state);
            }
            CodemodLang::Dynamic(lang) => {
                1u8.hash(state);
                lang.hash(state);
            }
        }
    }
}

impl fmt::Display for CodemodLang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodemodLang::Static(lang) => write!(f, "{}", lang),
            CodemodLang::Dynamic(lang) => write!(f, "{}", lang.name()),
        }
    }
}

impl fmt::Debug for CodemodLang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodemodLang::Static(lang) => write!(f, "CodemodLang::Static({:?})", lang),
            CodemodLang::Dynamic(lang) => write!(f, "CodemodLang::Dynamic({})", lang.name()),
        }
    }
}

impl FromStr for CodemodLang {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Try static languages first
        if let Ok(lang) = SupportLang::from_str(s) {
            return Ok(CodemodLang::Static(lang));
        }

        // Initialize dynamic parsers and try dynamic languages
        let _ = tree_sitter_loader::init();

        if let Ok(lang) = DynamicLang::from_str(s) {
            return Ok(CodemodLang::Dynamic(lang));
        }

        Err(format!("Unsupported language: {s}"))
    }
}

impl From<SupportLang> for CodemodLang {
    fn from(lang: SupportLang) -> Self {
        CodemodLang::Static(lang)
    }
}

impl From<DynamicLang> for CodemodLang {
    fn from(lang: DynamicLang) -> Self {
        CodemodLang::Dynamic(lang)
    }
}

impl Serialize for CodemodLang {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for CodemodLang {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let name = String::deserialize(deserializer)?;
        CodemodLang::from_str(&name).map_err(serde::de::Error::custom)
    }
}

impl Language for CodemodLang {
    fn pre_process_pattern<'q>(&self, query: &'q str) -> Cow<'q, str> {
        match self {
            CodemodLang::Static(lang) => lang.pre_process_pattern(query),
            CodemodLang::Dynamic(lang) => lang.pre_process_pattern(query),
        }
    }

    fn meta_var_char(&self) -> char {
        match self {
            CodemodLang::Static(lang) => lang.meta_var_char(),
            CodemodLang::Dynamic(lang) => lang.meta_var_char(),
        }
    }

    fn expando_char(&self) -> char {
        match self {
            CodemodLang::Static(lang) => lang.expando_char(),
            CodemodLang::Dynamic(lang) => lang.expando_char(),
        }
    }

    fn kind_to_id(&self, kind: &str) -> u16 {
        match self {
            CodemodLang::Static(lang) => lang.kind_to_id(kind),
            CodemodLang::Dynamic(lang) => lang.kind_to_id(kind),
        }
    }

    fn field_to_id(&self, field: &str) -> Option<u16> {
        match self {
            CodemodLang::Static(lang) => lang.field_to_id(field),
            CodemodLang::Dynamic(lang) => lang.field_to_id(field),
        }
    }

    fn from_path<P: AsRef<std::path::Path>>(path: P) -> Option<Self> {
        if let Some(lang) = SupportLang::from_path(path.as_ref()) {
            return Some(CodemodLang::Static(lang));
        }
        if let Some(lang) = DynamicLang::from_path(path.as_ref()) {
            return Some(CodemodLang::Dynamic(lang));
        }
        None
    }

    fn build_pattern(&self, builder: &PatternBuilder) -> Result<Pattern, PatternError> {
        match self {
            CodemodLang::Static(lang) => lang.build_pattern(builder),
            CodemodLang::Dynamic(lang) => lang.build_pattern(builder),
        }
    }
}

impl LanguageExt for CodemodLang {
    fn get_ts_language(&self) -> TSLanguage {
        match self {
            CodemodLang::Static(lang) => lang.get_ts_language(),
            CodemodLang::Dynamic(lang) => lang.get_ts_language(),
        }
    }
}
