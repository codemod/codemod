#!/usr/bin/env node
// Stand-in for `codemod-agent` (crates/orchestration-agent) in unit tests:
// reads the request line and, with CODEMOD_AGENT_SECRETS=stdin, the secrets
// line from stdin; answers on stdout. FAKE_AGENT_MODE:
//   echo (default)  succeeded, output.text = JSON { request, cwd, env, secrets }
//   hang            start FAKE_CHILDREN (default 1) children that never exit
//                   (own process groups with FAKE_DETACH_CHILDREN=1), write
//                   their pids to FAKE_PID_FILE, never answer
//   garbage         "{not json" on stdout
//   other-command   a completion for another command id
//   silent          exit 1 without output
// The FAKE_* settings may also come from `fake-agent.json` in the working
// directory (runtimes pass the runner an allowlisted environment only).
import { spawn } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";

if (existsSync("fake-agent.json")) {
  Object.assign(process.env, JSON.parse(readFileSync("fake-agent.json", "utf8")));
}

const input = readFileSync(0, "utf8");
const newline = input.indexOf("\n");
const request = JSON.parse(newline === -1 ? input : input.slice(0, newline));
const secretsLine = newline === -1 ? "" : input.slice(newline + 1);
const secrets =
  process.env.CODEMOD_AGENT_SECRETS === "stdin" ? JSON.parse(secretsLine || "null") : null;
const mode = process.env.FAKE_AGENT_MODE ?? "echo";
const reply = (completion) =>
  process.stdout.write(
    JSON.stringify({ protocolVersion: 8, commandId: request.commandId, ...completion }),
  );

if (mode === "hang") {
  const children = Array.from({ length: Number(process.env.FAKE_CHILDREN ?? "1") }, () =>
    spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"], {
      detached: process.env.FAKE_DETACH_CHILDREN === "1",
      stdio: "ignore",
    }),
  );
  writeFileSync(process.env.FAKE_PID_FILE, JSON.stringify(children.map((child) => child.pid)));
  setInterval(() => {}, 1000);
} else if (mode === "garbage") {
  process.stdout.write("{not json");
} else if (mode === "other-command") {
  process.stdout.write(
    JSON.stringify({ protocolVersion: 8, commandId: "other", status: "succeeded", output: {} }),
  );
} else if (mode === "silent") {
  process.exitCode = 1;
} else {
  reply({
    status: "succeeded",
    output: { text: JSON.stringify({ request, cwd: process.cwd(), env: process.env, secrets }) },
  });
}
