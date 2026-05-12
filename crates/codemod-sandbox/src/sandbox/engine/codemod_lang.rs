use ast_grep_core::matcher::{Pattern, PatternBuilder, PatternError};
use ast_grep_core::tree_sitter::{LanguageExt, StrDoc, TSLanguage};
use ast_grep_core::Language;
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
    Xml,
}

impl PartialEq for CodemodLang {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (CodemodLang::Static(a), CodemodLang::Static(b)) => a == b,
            (CodemodLang::Dynamic(a), CodemodLang::Dynamic(b)) => a == b,
            (CodemodLang::Xml, CodemodLang::Xml) => true,
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
            CodemodLang::Xml => {
                2u8.hash(state);
                "xml".hash(state);
            }
        }
    }
}

impl fmt::Display for CodemodLang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodemodLang::Static(lang) => write!(f, "{}", lang),
            CodemodLang::Dynamic(lang) => write!(f, "{}", lang.name()),
            CodemodLang::Xml => write!(f, "xml"),
        }
    }
}

impl fmt::Debug for CodemodLang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodemodLang::Static(lang) => write!(f, "CodemodLang::Static({:?})", lang),
            CodemodLang::Dynamic(lang) => write!(f, "CodemodLang::Dynamic({})", lang.name()),
            CodemodLang::Xml => write!(f, "CodemodLang::Xml"),
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

        if s.eq_ignore_ascii_case("xml") {
            return Ok(CodemodLang::Xml);
        }

        // Initialize dynamic parsers and try dynamic languages
        if let Err(e) = tree_sitter_loader::init() {
            eprintln!("Warning: failed to initialize dynamic tree-sitter parsers: {e}");
        }

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
            CodemodLang::Xml => pre_process_pattern(self.expando_char(), query),
        }
    }

    fn meta_var_char(&self) -> char {
        match self {
            CodemodLang::Static(lang) => lang.meta_var_char(),
            CodemodLang::Dynamic(lang) => lang.meta_var_char(),
            CodemodLang::Xml => '$',
        }
    }

    fn expando_char(&self) -> char {
        match self {
            CodemodLang::Static(lang) => lang.expando_char(),
            CodemodLang::Dynamic(lang) => lang.expando_char(),
            CodemodLang::Xml => '_',
        }
    }

    fn kind_to_id(&self, kind: &str) -> u16 {
        match self {
            CodemodLang::Static(lang) => lang.kind_to_id(kind),
            CodemodLang::Dynamic(lang) => lang.kind_to_id(kind),
            CodemodLang::Xml => self.get_ts_language().id_for_node_kind(kind, true),
        }
    }

    fn field_to_id(&self, field: &str) -> Option<u16> {
        match self {
            CodemodLang::Static(lang) => lang.field_to_id(field),
            CodemodLang::Dynamic(lang) => lang.field_to_id(field),
            CodemodLang::Xml => self
                .get_ts_language()
                .field_id_for_name(field)
                .map(|f| f.get()),
        }
    }

    fn from_path<P: AsRef<std::path::Path>>(path: P) -> Option<Self> {
        if let Some(lang) = SupportLang::from_path(path.as_ref()) {
            return Some(CodemodLang::Static(lang));
        }
        if let Some(ext) = path.as_ref().extension().and_then(|ext| ext.to_str()) {
            if matches!(
                ext.to_ascii_lowercase().as_str(),
                "xml" | "csproj" | "props" | "targets" | "config" | "resx" | "xaml"
            ) {
                return Some(CodemodLang::Xml);
            }
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
            CodemodLang::Xml => builder.build(|src| StrDoc::try_new(src, *self)),
        }
    }
}

impl LanguageExt for CodemodLang {
    fn get_ts_language(&self) -> TSLanguage {
        match self {
            CodemodLang::Static(lang) => lang.get_ts_language(),
            CodemodLang::Dynamic(lang) => lang.get_ts_language(),
            CodemodLang::Xml => tree_sitter_xml::LANGUAGE_XML.into(),
        }
    }
}

fn pre_process_pattern(expando: char, query: &str) -> Cow<'_, str> {
    let mut ret = Vec::with_capacity(query.len());
    let mut dollar_count = 0;
    for c in query.chars() {
        if c == '$' {
            dollar_count += 1;
            continue;
        }
        let need_replace = matches!(c, 'A'..='Z' | '_') || dollar_count == 3;
        let sigil = if need_replace { expando } else { '$' };
        ret.extend(std::iter::repeat_n(sigil, dollar_count));
        dollar_count = 0;
        ret.push(c);
    }
    let sigil = if dollar_count == 3 { expando } else { '$' };
    ret.extend(std::iter::repeat_n(sigil, dollar_count));
    Cow::Owned(ret.into_iter().collect())
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::CodemodLang;
    use ast_grep_core::{AstGrep, Language};

    #[test]
    fn parses_xml_language_name() {
        let lang: CodemodLang = "xml".parse().expect("xml language should parse");
        assert_eq!(lang.to_string(), "xml");
    }

    #[test]
    fn detects_xml_family_paths() {
        assert!(matches!(
            CodemodLang::from_path("test.csproj"),
            Some(CodemodLang::Xml)
        ));
        assert!(matches!(
            CodemodLang::from_path("App.config"),
            Some(CodemodLang::Xml)
        ));
    }

    #[test]
    fn parses_xml_documents() {
        let grep = AstGrep::new("<Project Sdk=\"Microsoft.NET.Sdk\" />", CodemodLang::Xml);
        let root = grep.root();
        assert_eq!(root.kind(), "document");
        assert_eq!(CodemodLang::Xml.expando_char(), '_');
    }

    #[test]
    fn xml_preprocessing_preserves_literal_dollar_content() {
        let processed = CodemodLang::Xml.pre_process_pattern("<Price>$5</Price>");
        assert_eq!(processed.as_ref(), "<Price>$5</Price>");
    }

    #[test]
    fn xml_preprocessing_rewrites_only_metavariables() {
        let processed = CodemodLang::Xml.pre_process_pattern(
            "<Project><Property>$VALUE</Property><Literal>$5</Literal></Project>",
        );
        assert_eq!(
            processed.as_ref(),
            "<Project><Property>_VALUE</Property><Literal>$5</Literal></Project>"
        );
    }
}
