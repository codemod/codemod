#!/usr/bin/env node
// Build crates/jssg-napi and copy the cdylib next to this package as
// `jssg-napi.<platform>-<arch>.node` (current platform only).
//
//   node scripts/build.mjs            # debug
//   node scripts/build.mjs --release  # release (what the benchmark uses)
//   node scripts/build.mjs --release --features timings   # benchmark phase timings
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync } from "node:fs";
import { resolve } from "node:path";

const release = process.argv.includes("--release");
const featuresAt = process.argv.indexOf("--features");
const features = featuresAt === -1 ? [] : ["--features", process.argv[featuresAt + 1] ?? ""];
const root = resolve(import.meta.dirname, "../../..");
const cargo = spawnSync(
  "cargo",
  ["build", "-p", "jssg-napi", ...(release ? ["--release"] : []), ...features],
  { cwd: root, stdio: "inherit" },
);
if (cargo.status !== 0) process.exit(cargo.status ?? 1);

const targetDir = resolve(root, process.env.CARGO_TARGET_DIR ?? "target");
const profile = release ? "release" : "debug";
const library = {
  darwin: "libjssg_napi.dylib",
  linux: "libjssg_napi.so",
  win32: "jssg_napi.dll",
}[process.platform];
if (library === undefined) throw new Error(`unsupported platform ${process.platform}`);
const from = resolve(targetDir, profile, library);
if (!existsSync(from)) throw new Error(`built library not found at ${from}`);
const to = resolve(import.meta.dirname, "..", `jssg-napi.${process.platform}-${process.arch}.node`);
copyFileSync(from, to);
process.stdout.write(`${from} -> ${to}\n`);
