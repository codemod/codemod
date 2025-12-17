import {
  ConsoleStdout,
  OpenFile,
  WASI,
  File as WasiFile,
} from "https://esm.sh/@bjorn3/browser_wasi_shim";
import factory from "../../../target/wasm-bindgen/codemod-sandbox_bg.js";

const path = new URL("../../../target/wasm-bindgen/codemod-sandbox_bg.wasm", import.meta.url);

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
const { instance } = await WebAssembly.instantiateStreaming(fetch(path), {
  "./codemod-sandbox_bg.js": codemodSandbox,
  wasi_snapshot_preview1: wasi.wasiImport,
});
codemodSandbox.__wbg_set_wasm(instance.exports);
wasi.start({ exports: instance.exports });
await codemodSandbox.initializeTreeSitter();
await codemodSandbox.setupParser("javascript", "/tree-sitter-javascript.wasm");

const result = await codemodSandbox.run_module(
  "eec75689-96ad-4f40-b9e6-3e65c4f2de4f",
  "default",
  "test",
  {},
  `
    import sg from "ast-grep";
    export default async function(inputs) {
      try {
          await sg.parseAsync("javascript", "const a = 1;");
          const root = await sg.parseAsync("javascript", "const a = 1;");
          console.log("Parsed successfully:", root.source());
          const node = root.root();
          console.log("Root node kind:", node.kind());

          console.log('kind', sg.kind("javascript", "number"));
          
          // Test basic node navigation
          console.log("Is named:", node.isNamed());
          console.log("Is leaf:", node.isLeaf());
          
          // Get the first child if available
          const child = node.child(0);
          if (child) {
              console.log("First child kind:", child.kind());
          }
          
          // Test getting node range
          const range = node.range();
          console.log("Node range:", range);
          
          // Test creating a matcher object for find operations
          const varDeclMatcher = {rule: { pattern: "const $NAME = $INIT;" }};
          const match = node.find(varDeclMatcher);
          if (match) {
              console.log("Found match with kind:", match.kind());
          }

          const numberNode = node.find({ rule: {kind : "number"} });
          console.log("Number node:", numberNode.text());
          const edit = numberNode.replace("'sss'");
          console.log("Edit:", edit);
          const newContent = node.commitEdits([edit]);
          console.log("New content:", newContent);

          return {
            inputs: "TEST",
            newContent: newContent,
          }
      } catch (e) {
          console.error("ast-grep error:", e.toString());
      }
    }
    `,
  JSON.stringify({
    args: "ARGS ARE HERE",
  }),
);

console.log({ result: JSON.parse(result) });
