#!/usr/bin/env node
// Stand-in for butterflow-execution-bridge in unit tests: reads the request
// file, writes a completion file. `shell` requests are echoed back inside a
// succeeded completion. `agent` requests echo { request, cwd, env, secrets }
// as text (secrets: the stdin JSON when CODEMOD_BRIDGE_SECRETS=stdin),
// or with FAKE_BRIDGE_MODE=hang start FAKE_CHILDREN (default 1) child processes
// that never exit, write their pids to FAKE_PID_FILE, and never answer; with
// FAKE_DETACH_CHILDREN=1 the children get their own process group, as the
// agent's bash tool session does. Adversarial agent modes: symlink-response
// (response.json is a symlink to FAKE_SENTINEL holding a forged success),
// directory-response, garbage-response. `jssg` requests answer a batch result shaped by
// FAKE_BRIDGE_MODE:
//   echo (default)  every file gets "// fake\n" appended and outputs
//                   { path, context, input } where context is the request
//                   context without files ({ targetRoot, artifact })
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
import { spawn } from "node:child_process";
import { mkdirSync, readFileSync, symlinkSync, writeFileSync } from "node:fs";

const [requestPath, responsePath] = process.argv.slice(2);
const request = JSON.parse(readFileSync(requestPath, "utf8"));
const mode = process.env.FAKE_BRIDGE_MODE ?? "echo";
const reply = (completion) =>
  writeFileSync(
    responsePath,
    JSON.stringify({ protocolVersion: 8, commandId: request.commandId, ...completion }),
  );

if (request.operation.kind === "shell") {
  reply({ status: "succeeded", output: { stdout: JSON.stringify({ request }) } });
} else if (request.operation.kind === "agent" && mode === "symlink-response") {
  // A sandbox escape attempt: the response path points at a file elsewhere
  // that holds a well-formed success.
  writeFileSync(
    process.env.FAKE_SENTINEL,
    JSON.stringify({
      protocolVersion: 8,
      commandId: request.commandId,
      status: "succeeded",
      output: { text: "forged" },
    }),
  );
  symlinkSync(process.env.FAKE_SENTINEL, responsePath);
} else if (request.operation.kind === "agent" && mode === "directory-response") {
  mkdirSync(responsePath);
} else if (request.operation.kind === "agent" && mode === "garbage-response") {
  writeFileSync(responsePath, "{not json");
} else if (request.operation.kind === "agent" && mode === "hang") {
  const children = Array.from({ length: Number(process.env.FAKE_CHILDREN ?? "1") }, () =>
    spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"], {
      detached: process.env.FAKE_DETACH_CHILDREN === "1",
      stdio: "ignore",
    }),
  );
  writeFileSync(process.env.FAKE_PID_FILE, JSON.stringify(children.map((child) => child.pid)));
  setInterval(() => {}, 1000);
} else if (request.operation.kind === "agent") {
  const secrets =
    process.env.CODEMOD_BRIDGE_SECRETS === "stdin" ? JSON.parse(readFileSync(0, "utf8")) : null;
  reply({
    status: "succeeded",
    output: { text: JSON.stringify({ request, cwd: process.cwd(), env: process.env, secrets }) },
  });
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
