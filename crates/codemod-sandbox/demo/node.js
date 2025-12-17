import * as fs from "node:fs";
import { ConsoleStdout, OpenFile, WASI, File as WasiFile } from "@bjorn3/browser_wasi_shim";
import factory from "../target/wasm-bindgen/codemod-sandbox_bg.js";

const path = new URL("../target/wasm-bindgen/codemod-sandbox_bg.wasm", import.meta.url);
const wasi = new WASI(
  [],
  [],
  [
    new OpenFile(new WasiFile([])), // stdin
    ConsoleStdout.lineBuffered((msg) => console.log(`[WASI stdout] ${msg}`)),
    ConsoleStdout.lineBuffered((msg) => console.warn(`[WASI stderr] ${msg}`)),
  ],
);
const codemodSandbox = factory();
const wasmBuffer = fs.readFileSync(decodeURI(path.pathname));
const { instance } = await WebAssembly.instantiate(wasmBuffer, {
  "./codemod-sandbox_bg.js": codemodSandbox,
  wasi_snapshot_preview1: wasi.wasiImport,
});
codemodSandbox.__wbg_set_wasm(instance.exports);
wasi.start({ exports: instance.exports });
const result = codemodSandbox.run_module(
  `
      export default function(inputs) {
        console.log("TEST");
        return {
          inputs
        };
      }
    `,
  JSON.stringify({
    args: "ARGS ARE HERE",
  }),
);
console.log({ result: JSON.parse(result) });
