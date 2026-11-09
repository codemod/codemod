#!/usr/bin/env node
// Writes a run's meta.json (stdout): what was measured and with what. Called
// by rounds.sh and overhead.sh; reads the binaries from the environment
// lib.sh exports and the run's arms.tsv / preflight.txt from --out.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";

const { values: a } = parseArgs({
  options: {
    out: { type: "string" },
    size: { type: "string" },
    files: { type: "string" },
    rounds: { type: "string" },
    cache: { type: "string" },
    seed: { type: "string" },
    repo: { type: "string" },
    "gate-hash": { type: "string" },
    "gate-files": { type: "string" },
    purge: { type: "string", default: "0" },
  },
});

const root = resolve(import.meta.dirname, "../..");
const run = (cmd, args, opts = {}) => {
  try {
    return execFileSync(cmd, args, {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
      ...opts,
    }).trim();
  } catch {
    return null;
  }
};
const git = (dir, ...args) => run("git", ["-C", dir, ...args]);
const sha256 = (path) =>
  existsSync(path) ? createHash("sha256").update(readFileSync(path)).digest("hex") : null;
const firstLine = (s) => (s === null ? null : s.split("\n")[0]);

const env = process.env;
const binaries = [
  ["codemod", env.CODEMOD_BIN],
  ["orchestration-quickjs-host", env.CODEMOD_QUICKJS_HOST_BIN],
  ["bench-exec", join(import.meta.dirname, "bin/bench-exec")],
]
  .filter(([, path]) => path)
  .map(([name, path]) => ({ name, path, sha256: sha256(path) }));

const out = a.out ? resolve(a.out) : null;
const arms =
  out && existsSync(join(out, "arms.tsv"))
    ? readFileSync(join(out, "arms.tsv"), "utf8")
        .split("\n")
        .filter(Boolean)
        .map((line) => {
          const [arm, command, concurrency] = line.split("\t");
          return { arm, command, concurrency };
        })
    : [];
const preflight =
  out && existsSync(join(out, "preflight.txt"))
    ? readFileSync(join(out, "preflight.txt"), "utf8")
    : null;

const repo = a.repo ? resolve(a.repo) : null;
const meta = {
  size: a.size,
  files: Number(a.files),
  rounds: Number(a.rounds),
  cache: a.cache,
  purge: a.purge === "1",
  seed: Number(a.seed),
  gate_hash: a["gate-hash"],
  gate_files: Number(a["gate-files"]),
  recorded_at: new Date().toISOString(),
  // sha/dirty as recorded when the run started (rounds.sh/sizes.sh export
  // them), plus the state now, so edits made during a run are visible.
  codemod: {
    sha: env.BENCH_START_SHA ?? git(root, "rev-parse", "HEAD"),
    branch: git(root, "rev-parse", "--abbrev-ref", "HEAD"),
    dirty: env.BENCH_START_DIRTY
      ? env.BENCH_START_DIRTY === "true"
      : (git(root, "status", "--porcelain") ?? "") !== "",
    sha_at_end: git(root, "rev-parse", "HEAD"),
    dirty_at_end: (git(root, "status", "--porcelain") ?? "") !== "",
  },
  input: {
    path: repo,
    sha: repo ? git(repo, "rev-parse", "HEAD") : null,
    tag: env.BENCH_INPUT_TAG ?? null,
    base_tag: env.VITE_TAG ?? null,
    base_sha: env.VITE_SHA ?? null,
  },
  versions: {
    node: process.version,
    rustc: firstLine(run("rustc", ["--version"])),
    hyperfine: firstLine(run("hyperfine", ["--version"])),
    codemod: env.CODEMOD_BIN ? firstLine(run(env.CODEMOD_BIN, ["--version"])) : null,
  },
  binaries,
  arms,
  workflow_cache: env.CODEMOD_WORKFLOW_CACHE ?? null,
  preflight,
};
process.stdout.write(`${JSON.stringify(meta, null, 2)}\n`);
