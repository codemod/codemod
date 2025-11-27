use crate::ast_grep::types::AstGrepError;
#[cfg(feature = "wasm")]
use crate::ast_grep::wasm_lang::WasmLang as SupportLang;
use ast_grep_config::{DeserializeEnv, RuleCore, SerializableRuleCore};
use ast_grep_core::{
    matcher::{KindMatcher, Matcher},
    meta_var::MetaVarEnv,
    Doc, Node, Pattern,
};
#[cfg(not(feature = "wasm"))]
use ast_grep_language::SupportLang;
use rquickjs::{Ctx, Exception, FromJs, Result as QResult, Value};
use std::borrow::Cow;

use super::serde::JsValue;

#[allow(clippy::large_enum_variant)]
pub enum JsMatcherRjs {
    Pattern(Pattern),
    Kind(KindMatcher),
    Config(RuleCore),
}

impl Matcher for JsMatcherRjs {
    fn match_node_with_env<'tree, D: Doc>(
        &self,
        node: Node<'tree, D>,
        env: &mut Cow<'_, MetaVarEnv<'tree, D>>,
    ) -> Option<Node<'tree, D>> {
        match self {
            JsMatcherRjs::Pattern(p) => p.match_node_with_env(node, env),
            JsMatcherRjs::Kind(k) => k.match_node_with_env(node, env),
            JsMatcherRjs::Config(c) => c.match_node_with_env(node, env),
        }
    }
}

// Convert a JavaScript value to an appropriate ast-grep matcher
pub fn convert_matcher<'js>(
    value: Value<'js>,
    lang: SupportLang,
    ctx: &Ctx<'js>,
) -> QResult<JsMatcherRjs> {
    if value.is_string() {
        let pattern_str = value.as_string().unwrap().to_string()?;
        let pattern = Pattern::new(&pattern_str, lang);
        return Ok(JsMatcherRjs::Pattern(pattern));
    } else if value.is_number() {
        let kind_id = value.as_number().unwrap() as u16;
        let kind_matcher = KindMatcher::from_id(kind_id);
        return Ok(JsMatcherRjs::Kind(kind_matcher));
    } else if value.is_object() {
        let js_value = JsValue::from_js(ctx, value)?;
        let serde_value: SerializableRuleCore = serde_json::from_value(js_value.0)
            .map_err(|e| Exception::throw_type(ctx, &e.to_string()))?;
        let env = DeserializeEnv::new(lang);
        let config = serde_value
            .get_matcher(env)
            .map_err(|e| Exception::throw_type(ctx, &e.to_string()))?;
        return Ok(JsMatcherRjs::Config(config));
    }

    Err(Exception::throw_type(
        ctx,
        "Matcher must be an object with a 'pattern' or 'kind' property",
    ))
}

pub fn detect_language_from_extension(extension: &str) -> Result<&'static str, AstGrepError> {
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
        "xml" => Ok("xml"),
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
