use ast_grep_core::matcher::{Pattern, PatternBuilder, PatternError};
use ast_grep_core::tree_sitter::{LanguageExt, StrDoc, TSLanguage};
use ast_grep_core::Language;
use ast_grep_language::SupportLang;
use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

/// A statically-linked language available without runtime parser loading.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum StaticLang {
    Builtin(SupportLang),
    Xml,
}

impl fmt::Display for StaticLang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StaticLang::Builtin(lang) => write!(f, "{lang}"),
            StaticLang::Xml => write!(f, "xml"),
        }
    }
}

impl fmt::Debug for StaticLang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StaticLang::Builtin(lang) => write!(f, "{lang:?}"),
            StaticLang::Xml => write!(f, "Xml"),
        }
    }
}

impl FromStr for StaticLang {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("xml") {
            return Ok(StaticLang::Xml);
        }

        SupportLang::from_str(s)
            .map(StaticLang::Builtin)
            .map_err(|e| format!("{e}"))
    }
}

impl From<SupportLang> for StaticLang {
    fn from(lang: SupportLang) -> Self {
        StaticLang::Builtin(lang)
    }
}

impl Language for StaticLang {
    fn pre_process_pattern<'q>(&self, query: &'q str) -> Cow<'q, str> {
        match self {
            StaticLang::Builtin(lang) => lang.pre_process_pattern(query),
            StaticLang::Xml => pre_process_pattern(self.expando_char(), query),
        }
    }

    fn meta_var_char(&self) -> char {
        match self {
            StaticLang::Builtin(lang) => lang.meta_var_char(),
            StaticLang::Xml => '$',
        }
    }

    fn expando_char(&self) -> char {
        match self {
            StaticLang::Builtin(lang) => lang.expando_char(),
            StaticLang::Xml => '_',
        }
    }

    fn kind_to_id(&self, kind: &str) -> u16 {
        match self {
            StaticLang::Builtin(lang) => lang.kind_to_id(kind),
            StaticLang::Xml => self.get_ts_language().id_for_node_kind(kind, true),
        }
    }

    fn field_to_id(&self, field: &str) -> Option<u16> {
        match self {
            StaticLang::Builtin(lang) => lang.field_to_id(field),
            StaticLang::Xml => self
                .get_ts_language()
                .field_id_for_name(field)
                .map(|f| f.get()),
        }
    }

    fn from_path<P: AsRef<std::path::Path>>(path: P) -> Option<Self> {
        if let Some(lang) = SupportLang::from_path(path.as_ref()) {
            return Some(StaticLang::Builtin(lang));
        }
        if let Some(ext) = path.as_ref().extension().and_then(|ext| ext.to_str()) {
            if matches!(
                ext.to_ascii_lowercase().as_str(),
                "xml" | "csproj" | "props" | "targets" | "config" | "resx" | "xaml"
            ) {
                return Some(StaticLang::Xml);
            }
        }
        None
    }

    fn build_pattern(&self, builder: &PatternBuilder) -> Result<Pattern, PatternError> {
        match self {
            StaticLang::Builtin(lang) => lang.build_pattern(builder),
            StaticLang::Xml => builder.build(|src| StrDoc::try_new(src, *self)),
        }
    }
}

impl LanguageExt for StaticLang {
    fn get_ts_language(&self) -> TSLanguage {
        match self {
            StaticLang::Builtin(lang) => lang.get_ts_language(),
            StaticLang::Xml => tree_sitter_xml::LANGUAGE_XML.into(),
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
    use super::StaticLang;
    use ast_grep_core::{AstGrep, Language};

    #[test]
    fn parses_xml_language_name() {
        let lang: StaticLang = "xml".parse().expect("xml language should parse");
        assert_eq!(lang.to_string(), "xml");
    }

    #[test]
    fn detects_xml_family_paths() {
        assert!(matches!(
            StaticLang::from_path("test.csproj"),
            Some(StaticLang::Xml)
        ));
        assert!(matches!(
            StaticLang::from_path("App.config"),
            Some(StaticLang::Xml)
        ));
    }

    #[test]
    fn parses_xml_documents() {
        let grep = AstGrep::new("<Project Sdk=\"Microsoft.NET.Sdk\" />", StaticLang::Xml);
        let root = grep.root();
        assert_eq!(root.kind(), "document");
        assert_eq!(StaticLang::Xml.expando_char(), '_');
    }

    #[test]
    fn xml_preprocessing_preserves_literal_dollar_content() {
        let processed = StaticLang::Xml.pre_process_pattern("<Price>$5</Price>");
        assert_eq!(processed.as_ref(), "<Price>$5</Price>");
    }

    #[test]
    fn xml_preprocessing_rewrites_only_metavariables() {
        let processed = StaticLang::Xml.pre_process_pattern(
            "<Project><Property>$VALUE</Property><Literal>$5</Literal></Project>",
        );
        assert_eq!(
            processed.as_ref(),
            "<Project><Property>_VALUE</Property><Literal>$5</Literal></Project>"
        );
    }
}
