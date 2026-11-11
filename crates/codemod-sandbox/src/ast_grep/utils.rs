use crate::ast_grep::sg_core::{
    compile_matcher, CoreLang as SupportLang, CoreMatcher, MatcherInput,
};
use crate::ast_grep::sg_node::throw_core_error;
use crate::ast_grep::types::AstGrepError;
use rquickjs::{Ctx, Exception, FromJs, Result as QResult, Value};
use std::str::FromStr;

use super::serde::JsValue;

/// Convert a JavaScript matcher argument (pattern string, kind id, or rule
/// config object) to a [`MatcherInput`].
pub fn js_matcher_input<'js>(value: Value<'js>, ctx: &Ctx<'js>) -> QResult<MatcherInput> {
    if value.is_string() {
        let pattern_str = value.as_string().unwrap().to_string()?;
        return Ok(MatcherInput::Pattern(pattern_str));
    } else if value.is_number() {
        let kind_id = value.as_number().unwrap() as u16;
        return Ok(MatcherInput::Kind(kind_id));
    } else if value.is_object() {
        let js_value = JsValue::from_js(ctx, value)?;
        return Ok(MatcherInput::Config(js_value.0));
    }

    Err(Exception::throw_type(
        ctx,
        "Matcher must be an object with a 'pattern' or 'kind' property",
    ))
}

// Convert a JavaScript value to an appropriate ast-grep matcher
pub fn convert_matcher<'js>(
    value: Value<'js>,
    lang: SupportLang,
    ctx: &Ctx<'js>,
) -> QResult<CoreMatcher> {
    let input = js_matcher_input(value, ctx)?;
    compile_matcher(input, lang).map_err(|e| throw_core_error(ctx, e))
}

pub fn detect_language_from_extension(extension: &str) -> Result<&'static str, AstGrepError> {
    detect_language_from_extension_with_xml_availability(extension, || {
        SupportLang::from_str("xml").is_ok()
    })
}

fn detect_language_from_extension_with_xml_availability(
    extension: &str,
    xml_parser_available: impl FnOnce() -> bool,
) -> Result<&'static str, AstGrepError> {
    match extension.to_lowercase().as_str() {
        "js" | "mjs" | "cjs" => Ok("javascript"),
        "ts" | "mts" | "cts" => Ok("typescript"),
        "tsx" => Ok("tsx"),
        "jsx" => Ok("javascript"), // JSX files often use .js extension
        "py" | "pyi" => Ok("python"),
        "rs" => Ok("rust"),
        "go" => Ok("go"),
        "java" => Ok("java"),
        "c" => Ok("c"),
        "cpp" | "cc" | "cxx" | "c++" => Ok("cpp"),
        "h" | "hpp" | "hxx" => Ok("cpp"), // Header files
        "cs" => Ok("csharp"),
        "php" => Ok("php"),
        "rb" => Ok("ruby"),
        "swift" => Ok("swift"),
        "kt" | "kts" => Ok("kotlin"),
        "scala" => Ok("scala"),
        "html" | "htm" => Ok("html"),
        "css" => Ok("css"),
        "scss" => Ok("scss"),
        "less" => Ok("less"),
        "json" => Ok("json"),
        "yaml" | "yml" => Ok("yaml"),
        "xml" | "csproj" | "props" | "targets" | "config" | "resx" | "xaml" => {
            if xml_parser_available() {
                Ok("xml")
            } else {
                Err(AstGrepError::Language(
                    "Unsupported file extension: XML parser is not available".to_string(),
                ))
            }
        }
        "sql" => Ok("sql"),
        "sh" | "bash" => Ok("bash"),
        "lua" => Ok("lua"),
        "dart" => Ok("dart"),
        "elixir" | "ex" | "exs" => Ok("elixir"),
        "elm" => Ok("elm"),
        "haskell" | "hs" => Ok("haskell"),
        "thrift" => Ok("thrift"),
        _ => Err(AstGrepError::Language(format!(
            "Unsupported file extension: {extension}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        detect_language_from_extension, detect_language_from_extension_with_xml_availability,
    };

    #[test]
    #[cfg(all(feature = "wasm", target_arch = "wasm32"))]
    fn detects_xml_family_extensions_when_parser_is_available() {
        for extension in [
            "xml", "csproj", "props", "targets", "config", "resx", "xaml",
        ] {
            assert_eq!(detect_language_from_extension(extension).unwrap(), "xml");
        }
    }

    #[test]
    #[cfg(all(feature = "native", not(target_arch = "wasm32")))]
    fn detects_native_xml_family_extensions_when_parser_is_available() {
        for extension in [
            "xml", "csproj", "props", "targets", "config", "resx", "xaml",
        ] {
            assert_eq!(
                detect_language_from_extension_with_xml_availability(extension, || true).unwrap(),
                "xml"
            );
        }
    }

    #[test]
    #[cfg(all(feature = "native", not(target_arch = "wasm32")))]
    fn rejects_native_xml_family_extensions_when_parser_is_unavailable() {
        let error = detect_language_from_extension_with_xml_availability("csproj", || false)
            .expect_err("csproj should require the XML parser");

        assert!(error
            .to_string()
            .contains("Unsupported file extension: XML parser is not available"));
    }

    #[test]
    fn rejects_unknown_extensions() {
        assert!(detect_language_from_extension("unknown").is_err());
    }
}
