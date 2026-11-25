// Repo health checks for codemod/codemod, run with the codemod CLI itself.
//
//   npx codemod@2.0.0-alpha.7 workflow run -w .codemod/workflows/health.ts --target . --dry-run
//
// Read-only: each check is a step that returns a `Check`, and the workflow
// returns one `Report` (see ./lib/report.ts). The run exits 1 when a check fails.

import { dependencyGraph, semver } from "codemod/deps";
import { forEachFile, step } from "codemod/workflow";
import { type Check, type Finding, finish } from "./lib/report.ts";

// ── 1. Single-version crates ────────────────────────────────────────────
// Replaces scripts/check-single-crossterm.sh (CI "Rust Test" job): two
// versions of a terminal crate in one binary fight over the terminal.

/** Crates the `codemod` binary must link exactly one version of. */
const SINGLE_VERSION_CRATES = ["crossterm"];

/** The crate whose graph counts, as `cargo tree -p codemod` (dev and build deps included). */
const ROOT_CRATE = "codemod";

// ── 2. `codemod@latest` pins ────────────────────────────────────────────
// `codemod@latest` is 1.x while v2 ships on the `alpha` tag, so a v2 doc or
// hint that says `npx codemod@latest` sends people to the wrong CLI.

/** Where users read commands: docs, CLI (and MCP) messages, the npm README, `init` templates. */
const USER_FACING = [
  "docs/**/*.mdx",
  "crates/cli/src/**/*.rs",
  "crates/mcp/src/**/*.rs",
  "crates/cli/src/templates/**",
  "crates/cli/npm/README.md",
];

/** The pin, with the command around it when there is one. */
const LATEST_PIN = /(?:(?:npx|bunx|pnpm dlx|yarn dlx|npm (?:i|install) -g)\s+)?codemod@latest\b/g;

/**
 * Lines allowed to say `codemod@latest`. Each pattern, and why:
 * - `1.x`: the line says that @latest is 1.x, which is exactly what we want people to read.
 */
const ALLOWED_LINES = [/\b1\.x\b/];

/** Rust test modules hold test data, not messages: `#[cfg(test)]` then `mod`. */
const RUST_TEST_MODULE = /^\s*#\[cfg\(test\)\]\s*\n\s*(?:pub(?:\(\w+\))?\s+)?mod\s/m;

export default async function main() {
  const crates = await step(
    "Single-version crates",
    dependencyGraph((g): Check => {
      const check = {
        id: "single-version-crates",
        title: `One version of ${SINGLE_VERSION_CRATES.join(", ")} in ${ROOT_CRATE}`,
      };
      const root = g.workspaces().find((ws) => ws.ecosystem === "cargo" && ws.name === ROOT_CRATE);
      if (!root) {
        const where = g.lockfiles.join(", ") || "no lockfile";
        return { ...check, status: "fail", findings: [{ message: `No ${ROOT_CRATE} crate in ${where}` }] };
      }

      const metrics: Record<string, number> = {};
      const findings: Finding[] = [];
      for (const name of SINGLE_VERSION_CRATES) {
        // Versions the root crate reaches, newest first, each with its shortest chain.
        const reached = g
          .nodes(name)
          .filter((node) => node.ecosystem === "cargo")
          .map((node) => ({ node, chain: g.chains({ from: root, to: node })[0] }))
          .filter((found) => found.chain)
          .sort((a, b) => semver.compare(b.node.version, a.node.version, { ecosystem: "cargo" }));
        metrics[`${name}_versions`] = reached.length;

        // The newest version stays; every other one is an extra.
        const kept = reached[0]?.node.version;
        for (const { node, chain } of reached.slice(1)) {
          const via = chain.map((crate) => crate.name).join(" > ");
          // Every crate that depends on this exact version, third-party ones included.
          const pulledBy = node.installedBy({ includeThirdParty: true }).map((edge) => String(edge.from));
          findings.push({
            message: `${node} next to ${name}@${kept}: ${via} (pulled in by ${pulledBy.join(", ")})`,
            file: "Cargo.lock",
          });
        }
      }
      return { ...check, status: findings.length ? "fail" : "pass", metrics, findings };
    }),
  );

  const pins = await step(
    "codemod@latest pins",
    forEachFile({ include: USER_FACING, containing: "codemod@latest" }, (file) => {
      let text = file.text;
      if (file.path.endsWith(".rs")) text = text.split(RUST_TEST_MODULE)[0];
      const found: Finding[] = [];
      text.split("\n").forEach((line, index) => {
        if (ALLOWED_LINES.some((allowed) => allowed.test(line))) return;
        for (const match of line.matchAll(LATEST_PIN)) {
          found.push({ message: `\`${match[0]}\` installs 1.x; pin the v2 version`, file: file.path, line: index + 1 });
        }
      });
      return found.length ? found : undefined;
    }),
  );

  const pinFindings = pins.outputs.flatMap((found) => [...found]);
  const latest: Check = {
    id: "latest-pins",
    title: "No `codemod@latest` in user-facing text",
    status: pinFindings.length ? "fail" : "pass",
    metrics: { pins: pinFindings.length, files: pins.outputs.length },
    findings: pinFindings,
  };

  return finish({ workflow: "health", checks: [crates, latest] });
}
