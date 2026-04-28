#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
mod ast_grep;
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
mod capabilities;
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
mod plugins;
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
mod sandbox;
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
mod utils;

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
pub use crate::sandbox::wasm_exports::*;

pub fn main() {}
