import { deepStrictEqual, strictEqual } from "node:assert";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test, { after, before } from "node:test";

import { SandboxedModule } from "../js/module.js";
import { NodeSandbox } from "../js/node.js";

const require = createRequire(import.meta.url);
const sandbox = new NodeSandbox();
let parserDirectory: string | undefined;

before(async () => {
  let parserPath = process.env.CODEMOD_TEST_JAVASCRIPT_WASM;
  if (!parserPath) {
    parserDirectory = await mkdtemp(join(tmpdir(), "codemod-wasm-parser-"));
    parserPath = join(parserDirectory, "tree-sitter-javascript.wasm");
    const response = await fetch(
      "https://github.com/tree-sitter/tree-sitter-javascript/releases/download/v0.25.0/tree-sitter-javascript.wasm",
    );
    strictEqual(response.ok, true, `JavaScript parser download failed: ${response.status}`);
    await writeFile(parserPath, Buffer.from(await response.arrayBuffer()));
  }
  await sandbox.initializeTreeSitter(() =>
    join(dirname(require.resolve("web-tree-sitter")), "tree-sitter.wasm"),
  );
  await sandbox.setupParser("javascript", parserPath);
});

after(async () => {
  if (parserDirectory) await rm(parserDirectory, { recursive: true });
});

async function run(code: string, source: string) {
  const module = new SandboxedModule(sandbox, {}, { test: code });
  return module.invoke("test", { source });
}

// CDMD-5018: WASM parser indices include padding excluded from the root node's text.
for (const source of [
  "var a = 1;\n",
  "\n\nvar a = 1;\n",
  "\uFEFFvar a = 1;\n",
  "\uFEFFa;",
  "\uFEFF\r\n\r\nvar a = 1;\r\n",
  "\uFEFF// 😀\nvar a = 1;\n",
]) {
  test(`WASM identifier edits preserve leading bytes in ${JSON.stringify(source)}`, async () => {
    const identifier = source.lastIndexOf("a");
    const start = Buffer.byteLength(source.slice(0, identifier));
    deepStrictEqual(
      await run(
        `import { parse } from "ast-grep";
        export default function({ source }) {
          const root = parse("javascript", source);
          const node = root.root().find({ rule: { kind: "identifier", regex: "^a$" } });
          const range = node.range();
          return {
            source: root.source(),
            start: range.start.index,
            end: range.end.index,
            output: root.root().commitEdits([node.replace("renamed")]),
          };
        }`,
        source,
      ),
      {
        source,
        start,
        end: start + 1,
        output: `${source.slice(0, identifier)}renamed${source.slice(identifier + 1)}`,
      },
    );
  });
}

test("WASM whole-declaration and non-root edits preserve their source boundaries", async () => {
  deepStrictEqual(
    await run(
      `import { parse } from "ast-grep";
      export default function({ source }) {
        const root = parse("javascript", source);
        const declaration = root.root().find({ rule: { kind: "variable_declaration" } });
        const block = root.root().find({ rule: { kind: "statement_block" } });
        const identifier = block.find({ rule: { kind: "identifier", regex: "^a$" } });
        return {
          whole: root.root().commitEdits([declaration.replace("const renamed = 1;")]),
          block: block.commitEdits([identifier.replace("renamed")]),
        };
      }`,
      "\uFEFF\nfunction f() { var a = 1; }\n",
    ),
    {
      whole: "\uFEFF\nfunction f() { const renamed = 1; }\n",
      block: "{ var renamed = 1; }",
    },
  );
});

test("WASM empty edits preserve BOM, CRLF, whitespace-only and empty documents", async () => {
  for (const source of ["\uFEFF\r\n\r\nconst x = 1;\r\n", " \t\n", ""]) {
    deepStrictEqual(
      await run(
        `import { parse } from "ast-grep";
        export default function({ source }) {
          const root = parse("javascript", source);
          return { source: root.source(), output: root.root().commitEdits([]) };
        }`,
        source,
      ),
      { source, output: source },
    );
  }
});
