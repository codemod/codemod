# Handoff: importing `.wasm` files from jssg transforms

Last updated Nov 17, 2026. Branch `handoff/wasm-loader` on `codemod/codemod`, based on Mo's `engine/code-first` at 6536221. No PR is open.

## TL;DR

- A jssg transform can now `import x from "./x.wasm"` a WebAssembly file shipped in its own package and call its functions. That's the whole feature.
- It exists so the react-to-solid codemod can ship oxc's React Compiler as a `.wasm` inside its package, instead of the compiler being built into codemod. Dev ruled that out on Nov 13: "i dont want react compiler to be built into codemod". Nothing in this branch names React.
- For that codemod it makes `extract` about 5x faster than Babel in QuickJS: median 68 ms per file against 348 ms. The codemod-side handoff has the numbers: `devagrawal09/react-to-solid-codemod`, branch `handoff/react-compiler-wasm`, `HANDOFF.md`.
- The costs:
  - wasmtime adds 13.8 MB to the CLI (3.6 MB gzipped);
  - about 1.2 s of one-time compile per run for a 3.5 MB module;
  - a new kind of code running in the CLI process.
- Before it ships, it needs a memory cap and cancellation. Both are listed under next steps.

## What a transform sees

```js
import analysis from "../lib/analysis.wasm";

const { exports, memory } = analysis.instantiate(); // fresh instance, its own memory

exports.add(40, 2);                 // 42. Exports are functions over i32 / i64 (BigInt) / f32 / f64
const n = memory.write(64, "wörld"); // UTF-8 into linear memory; returns bytes written
memory.readString(64, n);            // "wörld"
memory.read(64, n);                  // Uint8Array copy
memory.size();                       // bytes of linear memory
```

Rules the engine enforces:
- A module that **imports anything** is refused at load: "WebAssembly modules can't import anything; this one imports env.f". So a `.wasm` can only compute. It has no files, network, clock or host calls.
- Compiled code is cached per file for the life of the process, and compiled once even when several worker threads import it at the same moment.
- Each `instantiate()` call gives an instance with its own memory.
- A trap (a crash inside WASM) becomes a JS exception: `wasm analyze_utf8: …`.

Passing strings is a convention between the package's JS and its `.wasm`: allocate, write, call with pointer and length, read the result back. The engine doesn't check it.

## How it's wired (for a Rust reader new to this code)

| File | Change |
|---|---|
| `crates/codemod-sandbox/src/wasm_host/mod.rs` | New, about 370 lines with specs. It holds one shared wasmtime `Engine`, a per-path cache of compiled modules (`DashMap`), `declare()` (turns a `.wasm` file into a small JS module), and `WasmModule`, the internal module behind `instantiate()` |
| `crates/codemod-sandbox/src/sandbox/engine/quickjs_adapters.rs` | `QuickJSLoader` sends `*.wasm` files to `wasm_host::declare`. `with_codemod_modules!` registers the internal module, and `CodemodModuleResolver` answers its name |
| `crates/codemod-sandbox/src/lib.rs` | `pub mod wasm_host` |
| `crates/codemod-sandbox/Cargo.toml`, `Cargo.lock` | wasmtime 42 (Cranelift, no WASI), added to the `native` feature; `wat` as a dev-dependency for the specs |

Each `.wasm` import becomes this JS module, built in `declare()`:

```js
import { instantiate } from "codemod-host:wasm";
export default Object.freeze({ instantiate: () => instantiate("/abs/path/to/analysis.wasm") });
```

`codemod-host:wasm` is deliberately **not** a `codemod/...` module:
- it isn't in `CODEMOD_MODULES`;
- it isn't in the `codemod` npm package;
- it isn't something transforms import.

`CodemodModuleResolver` answers that one name, so the loader works everywhere `with_codemod_modules!` is used: transforms, the worker, the in-memory engine and selectors. `instantiate(path)` refuses any path the loader didn't compile, so a transform can't point it at an arbitrary file.

Everything is under `#[cfg(feature = "native")]`, so the browser/playground (`wasm` feature) build is unchanged and has no `.wasm` host.

**Why wasmtime 42 and not newer**:
- wasmtime 43 and later need `bumpalo` ≥ 3.20.2, but the CLI's rolldown dependency (oxc 0.96) pins `bumpalo = 3.19.0`.
- 42 needs Rust 1.91, so it builds on the repo's 1.97.1.

## Tested and not tested

Tested, on Mo's `engine/code-first` base, Nov 17:
- 5 specs in `wasm_host`:
  - a transform calls a function of a `.wasm` it imports;
  - it reads and writes memory as UTF-8;
  - each instance has its own memory;
  - a `.wasm` that imports from its host is refused;
  - `instantiate` refuses a file the transform didn't import.
- The full `codemod-sandbox` suite: 119 + 20 + 1 pass.
- Clippy and fmt are clean, and `cargo build -p codemod` succeeds.
- The release CLI ran react-to-solid's `extract` with its `.wasm` on a todo app (6 files) and 28 t3code files. Results matched the Nov 13 run on 34/34 files, at a median of 59 ms per t3code file and 5.7 s wall time.

Tested on the older #2485 base (5a33930), Nov 13:
- The release CLI running react-to-solid's `extract` with its 3.5 MB `.wasm` on a todo app and 28 t3code files.
- The facts matched the native build on 34/34 file summaries.

Not tested:
- The rest of the workspace's test suites; the CLI was built, not tested.
- A memory cap or cancellation, which don't exist yet.
- The browser build.
- wasmi or other runtimes.
- macOS or Windows.

## Decisions waiting on Dev

1. **Propose this to Mo?** Recommendation: yes, with the numbers, framed as a capability for codemod packages, not a React feature. Mo owns the engine and approves its PRs.
2. **Memory cap and cancellation before merge?** Recommendation: yes; see next steps. Without them, one file can grow an instance to gigabytes (a 4,000-line React file reached 2.2 GB), and `codemod/runtime`'s `isCanceled()` can't stop a running call.
3. **wasmtime vs a smaller interpreter (wasmi)?** Recommendation: keep wasmtime unless binary size is a hard limit. wasmi wasn't measured, but interpreters are typically several times slower on compute-heavy modules like this one.

## Next steps

1. Add `StoreLimits` (a per-instance memory cap) and epoch interruption (wired to the cancel flag) in `wasm_host/mod.rs`, each with a spec.
2. Enable wasmtime's `cache` feature to remove the ~1.2 s compile on later runs.
3. Decide on the browser build: map `.wasm` imports to the browser's `WebAssembly`, or document them as CLI-only.
4. If it ships, add a product-docs page in the `codemod` repo's `docs/`, covering `.wasm` imports in jssg transforms and the no-imports rule.
5. Consider WIT/component model later if more packages ship `.wasm`. It would type the ABI, at more engine weight.
