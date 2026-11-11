#!/usr/bin/env node
// Local dashboard for v2 (TypeScript) workflows. Zero dependencies: node:http
// serves the page and a small JSON API, and every run is a
// `codemod workflow run -w` child with --history and --events in its own
// folder. Node is only the dashboard; the workflows run in the codemod binary.
//
//   node workflow-lab/dashboard/server.mjs [--port 4319]
//
// CODEMOD_BIN: the codemod binary (default: target/debug/codemod, then PATH).
import { spawn } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  writeFileSync,
  copyFileSync,
} from "node:fs";
import { createServer } from "node:http";
import { homedir } from "node:os";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const lab = resolve(here, "..");
const repoRoot = resolve(lab, "..");
const workflowsDir = resolve(process.env.WORKFLOWS_DIR ?? join(lab, "workflows"));
const runsDir = resolve(process.env.RUNS_DIR ?? join(lab, ".runs"));
const portFlag = process.argv.indexOf("--port");
const port = Number(portFlag > 0 ? process.argv[portFlag + 1] : (process.env.PORT ?? 4319));
const codemodBin =
  process.env.CODEMOD_BIN ??
  (existsSync(join(repoRoot, "target/debug/codemod"))
    ? join(repoRoot, "target/debug/codemod")
    : "codemod");

/** Runs this server started; the only ones it can cancel. */
const children = new Map();

const readJson = (path, fallback) => {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch {
    return fallback;
  }
};
const expand = (path) => (path.startsWith("~/") ? join(homedir(), path.slice(2)) : path);

function listWorkflows() {
  return readdirSync(workflowsDir, { withFileTypes: true })
    .filter((d) => d.isDirectory() && existsSync(join(workflowsDir, d.name, "workflow.ts")))
    .map((d) => {
      // lab.json: what the engine has no place for yet (title, default target and input).
      const meta = readJson(join(workflowsDir, d.name, "lab.json"), {});
      return {
        name: d.name,
        title: meta.title ?? d.name,
        description: meta.description ?? "",
        target: meta.target ?? "",
        input: meta.input === undefined ? "" : JSON.stringify(meta.input),
        hasView: existsSync(join(workflowsDir, d.name, "view.js")),
        runs: listRuns(d.name).length,
      };
    });
}

function listRuns(workflow) {
  const dir = join(runsDir, workflow);
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .map((id) => withLiveStatus(readJson(join(dir, id, "run.json"), null)))
    .filter(Boolean)
    .sort((a, b) => b.startedAt - a.startedAt);
}

/** A "running" run with no child here was started by a dashboard that has since stopped. */
const withLiveStatus = (run) =>
  run?.status === "running" && !children.has(run.id) ? { ...run, status: "lost" } : run;

function readEvents(dir) {
  if (!existsSync(join(dir, "events.jsonl"))) return [];
  return readFileSync(join(dir, "events.jsonl"), "utf8")
    .split("\n")
    .filter(Boolean)
    .flatMap((line) => {
      try {
        return [JSON.parse(line)];
      } catch {
        return []; // a line still being written
      }
    });
}

/** Folds the event stream into one row per command. */
function stepsFrom(events) {
  const steps = new Map();
  const step = (id) => {
    if (!steps.has(id)) steps.set(id, { id, logs: "" });
    return steps.get(id);
  };
  for (const e of events) {
    if (e.type === "command.scheduled") {
      Object.assign(step(e.command.id), {
        name: e.command.runnable,
        kind: e.command.kind,
        status: "scheduled",
        scheduledAt: e.at,
        operation: summarize(e.command.operation),
      });
    } else if (e.type === "scheduler.queued") {
      Object.assign(step(e.commandId), { status: "queued" });
    } else if (e.type === "scheduler.admitted") {
      Object.assign(step(e.commandId), { status: "running", startedAt: e.at });
    } else if (e.type === "command.output") {
      step(e.commandId).logs += e.text;
    } else if (e.type === "command.completed" || e.type === "command.replayed") {
      const s = step(e.commandId);
      Object.assign(s, {
        status: e.type === "command.replayed" ? "replayed" : e.completion.status,
        endedAt: e.at,
        error: e.completion.error?.message,
        errorOutput: e.completion.error?.output,
        outputSize: Array.isArray(e.completion.output) ? e.completion.output.length : undefined,
      });
      if (s.kind === "shell" && typeof e.completion.output?.stdout === "string" && s.logs === "") {
        s.logs = e.completion.output.stdout;
      }
    }
  }
  return [...steps.values()];
}

function summarize(operation) {
  if (!operation) return "";
  if (operation.kind === "shell") return operation.command;
  if (operation.kind === "jssg") {
    const root = operation.target?.root ? ` in ${operation.target.root}` : "";
    return `${operation.language}${root}`;
  }
  return operation.kind;
}

function readRun(workflow, id) {
  const dir = join(runsDir, workflow, id);
  const run = withLiveStatus(readJson(join(dir, "run.json"), null));
  if (!run) return null;
  const events = readEvents(dir);
  // A replayed command has no `command.scheduled` event; its definition is in the history.
  const history = readJson(join(dir, "history.json"), { events: [] });
  const recorded = new Map(
    history.events.filter((e) => e.type === "scheduled").map((e) => [e.command.id, e.command]),
  );
  const steps = stepsFrom(events).map((step) => {
    const command = recorded.get(step.id);
    return step.kind || !command
      ? step
      : {
          ...step,
          name: command.runnable,
          kind: command.kind,
          operation: summarize(command.operation),
        };
  });
  return {
    ...run,
    steps,
    eventCount: events.length,
    output: readJson(join(dir, "output.json"), null),
    stderr: existsSync(join(dir, "stderr.log"))
      ? readFileSync(join(dir, "stderr.log"), "utf8").slice(-20_000)
      : "",
  };
}

/**
 * The engine replays every recorded completion, a cancelled one included, and
 * treats a scheduled command with no completion as `unknown`, so a resumed run
 * fails again at once. Dropping every command that did not succeed (both its
 * records) and the final output makes those commands run again. Only a
 * `cancelled` or `failed` command is dropped: those wrote nothing. An
 * `unknown` one may have, so it stays and the resume stops on it.
 */
function retryable(history) {
  if (!history) throw new Error("that run has no history to resume from");
  const retry = new Set(
    history.events
      .filter(
        (e) =>
          e.type === "completed" &&
          (e.completion.status === "cancelled" || e.completion.status === "failed"),
      )
      .map((e) => e.commandId),
  );
  return {
    ...history,
    events: history.events.filter(
      (e) =>
        (e.type === "scheduled" && !retry.has(e.command.id)) ||
        (e.type === "completed" && !retry.has(e.commandId)),
    ),
  };
}

function startRun({ workflow, target, input, resumeFrom }) {
  const workflowFile = join(workflowsDir, workflow, "workflow.ts");
  if (!existsSync(workflowFile)) throw new Error(`no workflow '${workflow}'`);
  const id =
    new Date().toISOString().replace(/[-:]/g, "").replace(/\..*/, "") +
    "-" +
    Math.random().toString(36).slice(2, 6);
  const dir = join(runsDir, workflow, id);
  mkdirSync(dir, { recursive: true });
  // Resume = a new run over a copy of the old history: recorded commands replay, the rest run.
  if (resumeFrom)
    writeFileSync(
      join(dir, "history.json"),
      JSON.stringify(
        retryable(readJson(join(runsDir, workflow, resumeFrom, "history.json"), null)),
      ),
    );

  const args = [
    "workflow",
    "run",
    "-w",
    workflowFile,
    "--target",
    expand(target),
    "--history",
    join(dir, "history.json"),
    "--events",
    join(dir, "events.jsonl"),
  ];
  if (input) args.push("--input", input);
  const run = {
    id,
    workflow,
    target,
    input: input || null,
    resumeFrom: resumeFrom ?? null,
    status: "running",
    startedAt: Date.now(),
  };
  writeFileSync(join(dir, "run.json"), JSON.stringify(run, null, 2));

  const child = spawn(codemodBin, args, { stdio: ["ignore", "pipe", "pipe"] });
  children.set(id, child);
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (chunk) => (stdout += chunk));
  child.stderr.on("data", (chunk) => {
    stderr += chunk;
    writeFileSync(join(dir, "stderr.log"), stderr);
  });
  const finish = (code, signal) => {
    children.delete(id);
    const final = readJson(join(dir, "run.json"), run);
    if (code === 0) writeFileSync(join(dir, "output.json"), stdout);
    final.status = final.cancelRequested ? "cancelled" : code === 0 ? "succeeded" : "failed";
    final.exitCode = code;
    final.signal = signal;
    final.endedAt = Date.now();
    // The engine prints its error last on stderr.
    if (code !== 0)
      final.error =
        stderr
          .replace(/\x1b\[[0-9;]*m/g, "")
          .trim()
          .split("\n")
          .filter(Boolean)
          .at(-1) ?? `exit ${code}`;
    writeFileSync(join(dir, "run.json"), JSON.stringify(final, null, 2));
  };
  child.on("error", (error) => {
    stderr += `${error.message}\n`;
    finish(127, null);
  });
  child.on("close", finish);
  return run;
}

function cancelRun(workflow, id) {
  const child = children.get(id);
  if (!child) throw new Error("run is not running in this dashboard");
  const path = join(runsDir, workflow, id, "run.json");
  writeFileSync(path, JSON.stringify({ ...readJson(path, {}), cancelRequested: true }, null, 2));
  // The engine records the step in flight as cancelled and writes its history.
  child.kill("SIGTERM");
  return { ok: true };
}

const TYPES = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".mjs": "text/javascript",
  ".css": "text/css",
};

async function body(req) {
  let text = "";
  for await (const chunk of req) text += chunk;
  return text ? JSON.parse(text) : {};
}

const server = createServer(async (req, res) => {
  const url = new URL(req.url, "http://localhost");
  const send = (status, value, type = "application/json") => {
    res.writeHead(status, { "content-type": type, "cache-control": "no-store" });
    res.end(type === "application/json" ? JSON.stringify(value) : value);
  };
  try {
    const parts = url.pathname.split("/").filter(Boolean);
    if (parts[0] === "api") {
      if (req.method === "GET" && parts[1] === "workflows") return send(200, listWorkflows());
      if (req.method === "GET" && parts[1] === "runs" && parts.length === 3)
        return send(200, listRuns(parts[2]));
      if (req.method === "GET" && parts[1] === "runs" && parts.length === 4) {
        const run = readRun(parts[2], parts[3]);
        return run ? send(200, run) : send(404, { error: "no such run" });
      }
      if (req.method === "POST" && parts[1] === "runs" && parts.length === 2)
        return send(200, startRun(await body(req)));
      if (req.method === "POST" && parts[1] === "runs" && parts[4] === "cancel")
        return send(200, cancelRun(parts[2], parts[3]));
      return send(404, { error: "not found" });
    }
    // A workflow's own results view lives next to it: workflows/<name>/view.js.
    if (parts[0] === "views" && parts.length === 2) {
      const file = join(workflowsDir, parts[1], "view.js");
      return existsSync(file) ? send(200, readFileSync(file, "utf8"), TYPES[".js"]) : send(404, "");
    }
    const file = join(here, "public", parts.length ? parts.join("/") : "index.html");
    if (!file.startsWith(join(here, "public")) || !existsSync(file))
      return send(404, "not found", "text/plain");
    return send(200, readFileSync(file), TYPES[extname(file)] ?? "application/octet-stream");
  } catch (error) {
    return send(400, { error: error.message });
  }
});

server.listen(port, "127.0.0.1", () => {
  console.log(`workflow dashboard: http://127.0.0.1:${port}`);
  console.log(`  workflows: ${workflowsDir}`);
  console.log(`  runs:      ${runsDir}`);
  console.log(`  codemod:   ${codemodBin}`);
});
