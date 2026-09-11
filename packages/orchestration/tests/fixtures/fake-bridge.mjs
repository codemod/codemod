#!/usr/bin/env node
// Stand-in for butterflow-execution-bridge in unit tests: reads the request
// file, writes a completion file. `exec` requests are echoed back inside a
// succeeded completion. `jssg` requests answer a batch result shaped by
// FAKE_BRIDGE_MODE:
//   echo (default)  every file gets "// fake\n" appended and outputs
//                   { path, context, input }
//   fail            a failed completion, nothing else
//   hang            never answers (cancellation tests)
//   escape          every file renames to "../escaped.ts"
//   escape-symlink  every file renames to "linkdir/out.ts"
//   escape-absolute a secondary edit targets an absolute path
//   conflict        every file renames to "same.ts"
//   rename          every file renames "x.ts" to "x.moved.ts"
//   rename-twice    a.ts also renames b.ts to "x.ts"; b.ts renames itself to "y.ts"
//   write-renamed   a.ts also renames b.ts to "b.moved.ts"; b.ts edits itself in place
//   secondary       a.ts also edits FAKE_SECONDARY_PATH
import { readFileSync, writeFileSync } from "node:fs";

const [requestPath, responsePath] = process.argv.slice(2);
const request = JSON.parse(readFileSync(requestPath, "utf8"));
const mode = process.env.FAKE_BRIDGE_MODE ?? "echo";
const reply = (completion) =>
  writeFileSync(
    responsePath,
    JSON.stringify({ protocolVersion: 4, commandId: request.commandId, ...completion }),
  );

if (request.operation.kind === "exec") {
  reply({ status: "succeeded", output: { stdout: JSON.stringify({ request }) } });
} else if (mode === "hang") {
  setInterval(() => {}, 1000);
} else if (mode === "fail") {
  reply({ status: "failed", error: { message: "scripted transform failure" } });
} else {
  const { files, ...context } = request.context;
  const edit = (path, content, renameTo) =>
    renameTo === undefined ? { path, content } : { path, content, renameTo };
  const outcome = (file) => {
    const { path, content } = file;
    switch (mode) {
      case "escape":
        return { path, edits: [edit(path, content, "../escaped.ts")] };
      case "escape-symlink":
        return { path, edits: [edit(path, content, "linkdir/out.ts")] };
      case "escape-absolute":
        return { path, edits: [edit("/etc/passwd", "pwned")] };
      case "conflict":
        return { path, edits: [edit(path, content, "same.ts")] };
      case "rename":
        return { path, edits: [edit(path, content, path.replace(/\.ts$/u, ".moved.ts"))] };
      case "rename-twice":
        return {
          path,
          edits:
            path === "a.ts"
              ? [edit(path, content), edit("b.ts", "b\n", "x.ts")]
              : [edit(path, content, "y.ts")],
        };
      case "write-renamed":
        return {
          path,
          edits:
            path === "a.ts"
              ? [edit(path, content), edit("b.ts", "b\n", "b.moved.ts")]
              : [edit(path, `${content}// b\n`)],
        };
      case "secondary":
        return {
          path,
          edits: [
            edit(path, `${content}// primary ${path}\n`),
            ...(path === "a.ts" ? [edit(process.env.FAKE_SECONDARY_PATH, "// secondary\n")] : []),
          ],
        };
      default:
        return {
          path,
          edits: [edit(path, `${content}// fake\n`)],
          output: { path, context, input: request.operation.input ?? null },
        };
    }
  };
  reply({ status: "succeeded", output: { files: files.map(outcome) } });
}
