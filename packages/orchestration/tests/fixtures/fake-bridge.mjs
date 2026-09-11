#!/usr/bin/env node
// Stand-in for butterflow-execution-bridge in unit tests.
//
// One-shot mode (`<request> <response>`): echoes the request inside a
// succeeded completion so tests can inspect the exec wire.
//
// Worker mode (`--jssg-worker`): speaks the JSONL protocol. FAKE_BRIDGE_MODE
// selects scripted behavior:
//   echo (default)  transform appends "// fake\n" and outputs { path, open, indexed }
//   fail-second     the second transform answers a recoverable error
//   fatal           the first transform answers a fatal error and exits 3
//   hang            transforms never answer (cancellation tests)
//   escape          every primary result renames to "../escaped.ts"
//   escape-symlink  every primary result renames to "linkdir/out.ts"
//   escape-secondary  a secondary result targets an absolute path
//   conflict        every primary result renames to "same.ts"
//   rename          every primary result renames "x.ts" to "x.moved.ts"
//   secondary       a.ts also edits FAKE_SECONDARY_PATH; every primary appends "// primary <path>\n"
import { readFileSync, writeFileSync } from "node:fs";
import { createInterface } from "node:readline";

const [first, second] = process.argv.slice(2);
if (first === "--jssg-worker") worker();
else oneShot(first, second);

function oneShot(requestPath, responsePath) {
  const request = JSON.parse(readFileSync(requestPath, "utf8"));
  writeFileSync(
    responsePath,
    JSON.stringify({
      protocolVersion: 3,
      commandId: request.commandId,
      status: "succeeded",
      output: { stdout: JSON.stringify({ request }) },
    }),
  );
}

function worker() {
  const mode = process.env.FAKE_BRIDGE_MODE ?? "echo";
  const send = (message) => process.stdout.write(`${JSON.stringify(message)}\n`);
  const modified = (content, renameTo) =>
    renameTo === undefined
      ? { kind: "modified", content }
      : { kind: "modified", content, renameTo };
  let open;
  let count = 0;
  const indexed = [];
  const rl = createInterface({ input: process.stdin });
  rl.on("line", (line) => {
    const message = JSON.parse(line);
    switch (message.type) {
      case "open": {
        open = message;
        const semantic = message.semanticAnalysis;
        send({
          type: "opened",
          protocolVersion: 3,
          extensions: [".ts", ".js"],
          semanticMode:
            semantic === undefined ? null : typeof semantic === "string" ? semantic : semantic.mode,
        });
        break;
      }
      case "index":
        indexed.push(message.path);
        send({ type: "indexed" });
        break;
      case "transform": {
        count += 1;
        const { path, content } = message;
        if (mode === "hang") return;
        if (mode === "fatal") {
          send({ type: "error", message: "scripted fatal failure", fatal: true });
          process.exit(3);
        }
        if (mode === "fail-second" && count === 2) {
          send({ type: "error", message: "scripted transform failure", fatal: false });
          return;
        }
        const transformed = (primary, secondary = [], output) =>
          send({
            type: "transformed",
            result: output === undefined ? { primary, secondary } : { primary, secondary, output },
          });
        if (mode === "escape") return transformed(modified(content, "../escaped.ts"));
        if (mode === "escape-symlink") return transformed(modified(content, "linkdir/out.ts"));
        if (mode === "escape-secondary") {
          return transformed({ kind: "unmodified" }, [
            { path: "/etc/passwd", result: modified("pwned") },
          ]);
        }
        if (mode === "conflict") return transformed(modified(content, "same.ts"));
        if (mode === "rename") {
          return transformed(modified(content, path.replace(/\.ts$/u, ".moved.ts")));
        }
        if (mode === "secondary") {
          const secondary =
            path === "a.ts"
              ? [{ path: process.env.FAKE_SECONDARY_PATH, result: modified("// secondary\n") }]
              : [];
          return transformed(modified(`${content}// primary ${path}\n`), secondary);
        }
        return transformed(modified(`${content}// fake\n`), [], {
          path,
          open: {
            script: open.script,
            scriptRoot: open.scriptRoot,
            language: open.language,
            targetRoot: open.targetRoot,
            semanticAnalysis: open.semanticAnalysis ?? null,
            input: open.input ?? null,
          },
          indexed: [...indexed],
        });
      }
      case "close":
        send({ type: "closed" });
        process.exit(0);
    }
  });
  rl.on("close", () => process.exit(0));
}
