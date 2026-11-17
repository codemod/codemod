//! WebAssembly modules a codemod ships in its own package, imported like any other file:
//!
//! ```js
//! import analysis from "./analysis.wasm";
//! const { exports, memory } = analysis.instantiate();
//! const len = exports.run(ptr, n);
//! const out = memory.readString(ptr, len);
//! ```
//!
//! The engine knows nothing about what a module does. A module may import nothing (no WASI,
//! no host functions), so it can only compute over bytes the transform copies in. Compiled code
//! is shared across contexts and cached per file for the life of the process; each
//! `instantiate()` gets its own linear memory, freed when the transform drops the instance.

use dashmap::DashMap;
use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::prelude::Rest;
use rquickjs::{
    module, BigInt, Ctx, Error, Exception, FromJs, Function, Module, Object, Result, TypedArray,
    Value,
};
use std::sync::{Arc, Mutex, OnceLock};
use wasmtime::{Engine, Instance, Memory, Store, Val, ValType};

/// The module the generated `.wasm` modules import `instantiate` from. It isn't one of the
/// `codemod/...` modules transforms import; it only instantiates files the loader read.
pub const HOST_MODULE: &str = "codemod-host:wasm";

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(Engine::default)
}

/// Compiled modules by resolved path. A path is only here if the module loader read it for
/// an `import`, so `instantiate` can't be pointed at arbitrary files.
fn compiled() -> &'static DashMap<String, wasmtime::Module> {
    static COMPILED: OnceLock<DashMap<String, wasmtime::Module>> = OnceLock::new();
    COMPILED.get_or_init(DashMap::new)
}

/// Declares the JavaScript module for an imported `.wasm` file. Its default export's
/// `instantiate()` returns a new instance with `exports` (the module's functions) and `memory`
/// (copying accessors).
pub fn declare<'js>(ctx: &Ctx<'js>, path: &str) -> Result<Module<'js, module::Declared>> {
    // The entry holds its shard's lock while compiling, so contexts that import the same file
    // at once wait for one compile instead of each compiling it.
    compiled()
        .entry(path.to_string())
        .or_try_insert_with(|| compile(path))?;
    let path_literal = serde_json::to_string(path).expect("a path serialises");
    let source = format!(
        "import {{ instantiate }} from \"{HOST_MODULE}\";\nexport default Object.freeze({{ instantiate: () => instantiate({path_literal}) }});\n"
    );
    Module::declare(ctx.clone(), path, source)
}

fn compile(path: &str) -> Result<wasmtime::Module> {
    let bytes = std::fs::read(path).map_err(|_| Error::new_loading(path))?;
    let module = wasmtime::Module::new(engine(), &bytes)
        .map_err(|e| Error::new_loading_message(path, format!("invalid WebAssembly: {e}")))?;
    if let Some(import) = module.imports().next() {
        return Err(Error::new_loading_message(
            path,
            format!(
                "WebAssembly modules can't import anything; this one imports {}.{}",
                import.module(),
                import.name()
            ),
        ));
    }
    Ok(module)
}

pub struct WasmModule;

impl ModuleDef for WasmModule {
    fn declare(declare: &Declarations) -> Result<()> {
        declare.declare("instantiate")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        exports.export("instantiate", Function::new(ctx.clone(), instantiate)?)?;
        Ok(())
    }
}

type Shared = Arc<Mutex<Store<()>>>;

fn instantiate<'js>(ctx: Ctx<'js>, path: String) -> Result<Object<'js>> {
    let Some(module) = compiled().get(&path).map(|m| m.clone()) else {
        return Err(Exception::throw_message(
            &ctx,
            "only .wasm files the transform imports can be instantiated",
        ));
    };
    let mut store = Store::new(engine(), ());
    let instance = Instance::new(&mut store, &module, &[])
        .map_err(|e| Exception::throw_message(&ctx, &format!("{path}: {e}")))?;
    let store: Shared = Arc::new(Mutex::new(store));

    let functions = Object::new(ctx.clone())?;
    let mut memory = None;
    {
        let mut guard = store.lock().unwrap();
        let names: Vec<String> = module.exports().map(|e| e.name().to_string()).collect();
        for name in names {
            if let Some(func) = instance.get_func(&mut *guard, &name) {
                let ty = func.ty(&*guard);
                let params: Vec<ValType> = ty.params().collect();
                let results = ty.results().len();
                let store = Arc::clone(&store);
                let label = name.clone();
                let call = move |ctx: Ctx<'js>, args: Rest<Value<'js>>| -> Result<Value<'js>> {
                    let inputs = params
                        .iter()
                        .enumerate()
                        .map(|(i, ty)| to_wasm(&ctx, args.0.get(i), ty))
                        .collect::<Result<Vec<_>>>()?;
                    let mut outputs = vec![Val::I32(0); results];
                    let mut guard = store.lock().unwrap();
                    func.call(&mut *guard, &inputs, &mut outputs).map_err(|e| {
                        Exception::throw_message(&ctx, &format!("wasm {label}: {e}"))
                    })?;
                    drop(guard);
                    match outputs.as_slice() {
                        [] => Ok(Value::new_undefined(ctx)),
                        [one] => from_wasm(&ctx, one),
                        many => {
                            let array = rquickjs::Array::new(ctx.clone())?;
                            for (i, v) in many.iter().enumerate() {
                                array.set(i, from_wasm(&ctx, v)?)?;
                            }
                            Ok(array.into_value())
                        }
                    }
                };
                functions.set(
                    name.as_str(),
                    Function::new(ctx.clone(), call)?.with_name(&name)?,
                )?;
            } else if memory.is_none() {
                memory = instance.get_memory(&mut *guard, &name);
            }
        }
    }

    let result = Object::new(ctx.clone())?;
    result.set("exports", functions)?;
    if let Some(memory) = memory {
        result.set("memory", memory_object(&ctx, &store, memory)?)?;
    }
    Ok(result)
}

/// `read(ptr, len)` returns a copy as a Uint8Array, `readString(ptr, len)` decodes UTF-8,
/// `write(ptr, data)` copies a Uint8Array or a string (as UTF-8) in and returns the bytes
/// written, and `size()` is the memory's current size in bytes.
fn memory_object<'js>(ctx: &Ctx<'js>, store: &Shared, memory: Memory) -> Result<Object<'js>> {
    let object = Object::new(ctx.clone())?;

    let s = Arc::clone(store);
    let read = move |ctx: Ctx<'js>, ptr: u32, len: u32| -> Result<TypedArray<'js, u8>> {
        let bytes = read_bytes(&ctx, &s, memory, ptr, len)?;
        TypedArray::new(ctx, bytes)
    };
    object.set("read", Function::new(ctx.clone(), read)?)?;

    let s = Arc::clone(store);
    let read_string = move |ctx: Ctx<'js>, ptr: u32, len: u32| -> Result<String> {
        let bytes = read_bytes(&ctx, &s, memory, ptr, len)?;
        String::from_utf8(bytes)
            .map_err(|_| Exception::throw_message(&ctx, "wasm memory: not UTF-8"))
    };
    object.set("readString", Function::new(ctx.clone(), read_string)?)?;

    let s = Arc::clone(store);
    let write = move |ctx: Ctx<'js>, ptr: u32, data: Value<'js>| -> Result<usize> {
        if let Some(text) = data.as_string() {
            let text = text.to_string()?;
            return write_bytes(&ctx, &s, memory, ptr, text.as_bytes());
        }
        let array = TypedArray::<u8>::from_js(&ctx, data).map_err(|_| {
            Exception::throw_type(&ctx, "wasm memory.write takes a string or Uint8Array")
        })?;
        let bytes = array
            .as_bytes()
            .ok_or_else(|| Exception::throw_type(&ctx, "wasm memory.write: detached buffer"))?;
        write_bytes(&ctx, &s, memory, ptr, bytes)
    };
    object.set("write", Function::new(ctx.clone(), write)?)?;

    let s = Arc::clone(store);
    let size = move || -> usize { memory.data_size(&*s.lock().unwrap()) };
    object.set("size", Function::new(ctx.clone(), size)?)?;
    Ok(object)
}

fn read_bytes(
    ctx: &Ctx<'_>,
    store: &Shared,
    memory: Memory,
    ptr: u32,
    len: u32,
) -> Result<Vec<u8>> {
    let mut bytes = vec![0; len as usize];
    memory
        .read(&*store.lock().unwrap(), ptr as usize, &mut bytes)
        .map_err(|_| Exception::throw_range(ctx, "wasm memory read out of bounds"))?;
    Ok(bytes)
}

fn write_bytes(
    ctx: &Ctx<'_>,
    store: &Shared,
    memory: Memory,
    ptr: u32,
    bytes: &[u8],
) -> Result<usize> {
    memory
        .write(&mut *store.lock().unwrap(), ptr as usize, bytes)
        .map_err(|_| Exception::throw_range(ctx, "wasm memory write out of bounds"))?;
    Ok(bytes.len())
}

fn to_wasm<'js>(ctx: &Ctx<'js>, value: Option<&Value<'js>>, ty: &ValType) -> Result<Val> {
    let number = |v: Option<&Value<'js>>| -> Result<f64> {
        v.and_then(|v| v.as_number())
            .ok_or_else(|| Exception::throw_type(ctx, "wasm functions take numbers"))
    };
    Ok(match ty {
        ValType::I32 => Val::I32(number(value)? as i64 as i32),
        ValType::I64 => match value.and_then(|v| v.as_big_int()) {
            Some(big) => Val::I64(big.clone().to_i64()?),
            None => Val::I64(number(value)? as i64),
        },
        ValType::F32 => Val::F32((number(value)? as f32).to_bits()),
        ValType::F64 => Val::F64(number(value)?.to_bits()),
        _ => {
            return Err(Exception::throw_type(
                ctx,
                "wasm functions take i32, i64, f32 or f64",
            ))
        }
    })
}

fn from_wasm<'js>(ctx: &Ctx<'js>, value: &Val) -> Result<Value<'js>> {
    Ok(match value {
        Val::I32(v) => Value::new_int(ctx.clone(), *v),
        Val::I64(v) => BigInt::from_i64(ctx.clone(), *v)?.into_value(),
        Val::F32(bits) => Value::new_float(ctx.clone(), f32::from_bits(*bits) as f64),
        Val::F64(bits) => Value::new_float(ctx.clone(), f64::from_bits(*bits)),
        _ => Value::new_undefined(ctx.clone()),
    })
}

#[cfg(test)]
mod tests {
    use crate::sandbox::engine::codemod_lang::CodemodLang;
    use crate::sandbox::engine::{
        execute_codemod_with_quickjs, ExecutionResult, JssgExecutionOptions,
    };
    use crate::sandbox::resolvers::oxc_resolver::OxcResolver;
    use ast_grep_language::SupportLang;
    use std::sync::Arc;

    const ADDER: &str = r#"(module
      (memory (export "memory") 1)
      (data (i32.const 8) "héllo")
      (func (export "add") (param i32 i32) (result i32) (i32.add (local.get 0) (local.get 1))))"#;

    /// Runs `transform` over a one-line file next to the given `.wasm` files (as WAT) and
    /// returns what it returned, or the error message.
    async fn run(transform: &str, wasm: &[(&str, &str)]) -> Result<String, String> {
        let dir = tempfile::TempDir::new().unwrap();
        for (name, text) in wasm {
            std::fs::write(dir.path().join(name), wat::parse_str(text).unwrap()).unwrap();
        }
        let script = dir.path().join("transform.js");
        std::fs::write(&script, transform).unwrap();
        let file = dir.path().join("input.js");
        std::fs::write(&file, "x;\n").unwrap();
        let options = JssgExecutionOptions {
            script_path: &script,
            resolver: Arc::new(OxcResolver::new(dir.path().to_path_buf(), None).unwrap()),
            language: CodemodLang::Static(SupportLang::JavaScript),
            file_path: &file,
            content: "x;\n",
            selector_config: None,
            params: None,
            matrix_values: None,
            capabilities: None,
            semantic_provider: None,
            metrics_context: None,
            llm_request_handler: None,
            runtime_event_callback: None,
            cancellation_flag: None,
            test_mode: false,
            dry_run: true,
            target_directory: dir.path(),
            pending_files: None,
        };
        match execute_codemod_with_quickjs(options).await {
            Ok(result) => match result.primary {
                ExecutionResult::Modified(modified) => Ok(modified.content),
                other => Err(format!("{other:?}")),
            },
            Err(e) => Err(e.to_string()),
        }
    }

    #[tokio::test]
    async fn a_transform_calls_a_function_of_a_wasm_file_it_imports() {
        let transform = r#"import adder from "./adder.wasm";
export default () => String(adder.instantiate().exports.add(40, 2));"#;
        assert_eq!(
            run(transform, &[("adder.wasm", ADDER)]).await.unwrap(),
            "42"
        );
    }

    #[tokio::test]
    async fn a_transform_reads_and_writes_a_wasm_modules_memory_as_utf8() {
        let transform = r#"import adder from "./adder.wasm";
export default () => {
  const { memory } = adder.instantiate();
  const written = memory.write(64, "wörld");
  return `${memory.readString(8, 6)} ${memory.readString(64, written)}`;
};"#;
        assert_eq!(
            run(transform, &[("adder.wasm", ADDER)]).await.unwrap(),
            "héllo wörld"
        );
    }

    #[tokio::test]
    async fn each_instance_of_a_wasm_file_has_its_own_memory() {
        let transform = r#"import adder from "./adder.wasm";
export default () => {
  adder.instantiate().memory.write(8, "jello");
  return adder.instantiate().memory.readString(8, 6);
};"#;
        assert_eq!(
            run(transform, &[("adder.wasm", ADDER)]).await.unwrap(),
            "héllo"
        );
    }

    #[tokio::test]
    async fn a_wasm_file_that_imports_from_its_host_is_refused() {
        let wants_host = r#"(module (import "env" "log" (func)))"#;
        let transform = r#"import m from "./wants-host.wasm";
export default () => "loaded";"#;
        let error = run(transform, &[("wants-host.wasm", wants_host)])
            .await
            .unwrap_err();
        assert!(error.contains("can't import anything"), "{error}");
    }

    #[tokio::test]
    async fn instantiate_refuses_a_wasm_file_the_transform_did_not_import() {
        let transform = r#"import { instantiate } from "codemod-host:wasm";
export default () => { instantiate("/etc/never-imported.wasm"); return "instantiated"; };"#;
        let error = run(transform, &[]).await.unwrap_err();
        assert!(
            error.contains("only .wasm files the transform imports can be instantiated"),
            "{error}"
        );
    }
}
