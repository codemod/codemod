use rquickjs::{
    AsyncContext, AsyncRuntime, CatchResultExt, Error as RquickjsError, Function, Module, Value,
    async_with,
    loader::{BuiltinLoader, BuiltinResolver, ModuleLoader},
};
use thiserror::Error;
use wasm_bindgen::prelude::*;

use crate::ast_grep::AstGrepModule;
use crate::ast_grep::{
    scanner::scan_content,
    wasm_utils::{WasmMatch, convert_to_debug_node, dump_pattern_impl, try_get_rule_config},
};
use crate::capabilities::CapabilitiesModule;
use crate::plugins;
use crate::utils::transpiler;
use crate::{
    ast_grep::wasm_lang::{WasmDoc, WasmLang},
    utils::quickjs_utils::maybe_promise,
    utils::quickjs_wasm::quickjs_value_to_jsvalue,
};
use ast_grep_config::CombinedScan;
use ast_grep_core::AstGrep;

use std::collections::HashMap;

use web_tree_sitter_sg::TreeSitter;

#[derive(Debug, Error)]
enum Error {
    #[error("A QuickJS error occured: {0}")]
    QuickJS(#[from] RquickjsError),

    #[error("Error loading module: {0}")]
    Loading(String),

    #[error("Error evaluating module: {0}")]
    Evaluating(String),

    #[error("Error getting default export: {0}")]
    GettingDefaultExport(String),

    #[error("Error getting `default` function: {0}")]
    GettintDefaultFunction(String),

    #[error("Error parsing input values: {0}")]
    ParsingInputValues(String),

    #[error("{0}")]
    CallingModuleFunction(String),
}

#[wasm_bindgen(js_name = initializeTreeSitter)]
pub async fn initialize_tree_sitter(
    locate_file: Option<js_sys::Function>,
) -> std::result::Result<(), JsError> {
    TreeSitter::init(locate_file).await
}

#[wasm_bindgen(js_name = setupParser)]
pub async fn setup_parser(
    lang_name: String,
    parser_path: String,
) -> std::result::Result<(), JsError> {
    WasmLang::set_current(&lang_name, &parser_path).await
}

macro_rules! add_capability_modules {
    ($resolver:expr, $loader:expr, $module_name:expr, $invocation_id:expr, $($capability:expr),+ $(,)?) => {
        $(
            $loader.add_module(
                concat!("@", $capability),
                format!(
                    "import {{ {} }} from \"{}\";\nexport default function(inputs) {{ return {}(\"{}\", inputs); }}\n",
                    $capability,
                    $module_name,
                    $capability,
                    $invocation_id
                )
            );
            $resolver.add_module(concat!("@", $capability));
        )+
    };
}

#[wasm_bindgen]
pub async fn run_module(
    invocation_id: String,
    method: String,
    name: String,
    modules: JsValue,
    code: String,
    json: String,
) -> std::result::Result<JsValue, JsError> {
    let mut resolver = BuiltinResolver::default();

    let capability_module_name = format!("codemod-{}", invocation_id);

    let mut capabilities_loader = BuiltinLoader::default();
    add_capability_modules!(
        resolver,
        capabilities_loader,
        capability_module_name,
        invocation_id,
        "fetch",
    );

    let mut peer_loader = BuiltinLoader::default();
    let object = js_sys::Object::from(modules);
    let entries = js_sys::Object::entries(&object);
    for i in 0..entries.length() {
        let entry = js_sys::Array::from(&entries.get(i));
        let peer = entry.get(0).as_string().unwrap_or_default();
        let code = entry.get(1).as_string().unwrap_or_default();
        let transpiled = transpiler::transpile(code, name.clone()).unwrap();
        if !peer.is_empty() && !transpiled.is_empty() && name != peer {
            let peer_js = format!("{}.js", peer);
            peer_loader.add_module(&peer, transpiled.clone());
            resolver.add_module(&peer);
            peer_loader.add_module(&peer_js, transpiled);
            resolver.add_module(&peer_js);
        }
    }
    resolver.add_module(capability_module_name.clone());

    // Register ast_grep module
    resolver.add_module("ast-grep");

    let loader = (
        capabilities_loader,
        peer_loader,
        ModuleLoader::default()
            .with_module(capability_module_name.clone(), CapabilitiesModule)
            .with_module("ast-grep", AstGrepModule),
    );

    let rt = AsyncRuntime::new()?;
    let ctx = AsyncContext::full(&rt).await?;

    ctx.with(|ctx| plugins::console::init(&ctx)).await?;
    ctx.with(|ctx| plugins::atob::init(&ctx)).await?;

    rt.set_loader(resolver, loader).await;

    let result = async_with!(ctx => |ctx| {

        // Load the module.
        let module = Module::declare(ctx.clone(), name, code)
            .catch(&ctx)
            .map_err(|e| Error::Loading(e.to_string()))?;

        // Evaluate module.
        let (evaluated, _) = module
            .eval()
            .catch(&ctx)
            .map_err(|e| Error::Evaluating(e.to_string()))?;
        while ctx.execute_pending_job() {}

        // Get the default export.
        let namespace = evaluated
            .namespace()
            .catch(&ctx)
            .map_err(|e| Error::GettingDefaultExport(e.to_string()))?;

        let func = namespace
            .get::<_, Function>(method)
            .catch(&ctx)
            .map_err(|e| Error::GettintDefaultFunction(e.to_string()))?;

        let inputs = ctx
            .json_parse(json)
            .catch(&ctx)
            .map_err(|e| Error::ParsingInputValues(e.to_string()))?;

        // Call it and return value.
        let result_obj: Value = maybe_promise(
            func.call((inputs,)).catch(&ctx)
            .map_err(|e| Error::CallingModuleFunction(e.to_string()))?
        ).await.catch(&ctx)
        .map_err(|e| Error::CallingModuleFunction(e.to_string()))?;

        quickjs_value_to_jsvalue(result_obj)
    })
    .await;

    rt.idle().await;

    result
}

#[wasm_bindgen(js_name = scanFind)]
pub fn scan_find(src: String, configs: Vec<JsValue>) -> std::result::Result<JsValue, JsError> {
    let lang = WasmLang::get_current();
    let mut rules = vec![];
    for config in configs {
        let finder = try_get_rule_config(config)?;
        rules.push(finder);
    }
    let combined = CombinedScan::new(rules.iter().collect());
    let doc = WasmDoc::try_new(src.clone(), lang).map_err(|e| JsError::new(&e.to_string()))?;
    let root = AstGrep::doc(doc);
    let ret: HashMap<_, _> = combined
        .scan(&root, false)
        .matches
        .into_iter()
        .map(|(rule, matches)| {
            let matches: Vec<_> = matches
                .into_iter()
                .map(|m| WasmMatch::from_match(m, rule))
                .collect();
            (rule.id.clone(), matches)
        })
        .collect();
    let ret = serde_wasm_bindgen::to_value(&ret).map_err(|e| JsError::new(&e.to_string()))?;
    Ok(ret)
}

#[wasm_bindgen(js_name = scanFix)]
pub fn scan_fix(src: String, configs: Vec<JsValue>) -> std::result::Result<String, JsError> {
    let lang = WasmLang::get_current();
    let mut rules = vec![];
    for config in configs {
        let finder = try_get_rule_config(config)?;
        rules.push(finder);
    }
    let combined = CombinedScan::new(rules.iter().collect());
    let doc = WasmDoc::try_new(src.clone(), lang).map_err(|e| JsError::new(&e.to_string()))?;
    let root = AstGrep::doc(doc);

    let file_path = "file_path";
    let apply_fixes = true;
    let result = scan_content(&root, &src, file_path.to_string(), &combined, apply_fixes)?;
    Ok(result.new_content)
}

#[wasm_bindgen(js_name = dumpASTNodes)]
pub fn dump_ast_nodes(src: String) -> std::result::Result<JsValue, JsError> {
    let lang = WasmLang::get_current();
    let doc = WasmDoc::try_new(src, lang).map_err(|e| JsError::new(&e.to_string()))?;
    let root = AstGrep::doc(doc);
    let debug_node = convert_to_debug_node(root.root());
    let ret =
        serde_wasm_bindgen::to_value(&debug_node).map_err(|e| JsError::new(&e.to_string()))?;
    Ok(ret)
}

#[wasm_bindgen(js_name = dumpPattern)]
pub fn dump_pattern(
    src: String,
    selector: Option<String>,
) -> std::result::Result<JsValue, JsError> {
    let dumped = dump_pattern_impl(src, selector)?;
    let ret = serde_wasm_bindgen::to_value(&dumped).map_err(|e| JsError::new(&e.to_string()))?;
    Ok(ret)
}

#[wasm_bindgen(main)]
pub fn main() {}
