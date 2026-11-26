pub mod project_discovery;
pub mod quickjs_utils;
pub mod transpiler;

#[cfg(feature = "native")]
pub mod js_syntax;

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
pub mod quickjs_wasm;

#[cfg(feature = "native")]
pub mod yaml;
