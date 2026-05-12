import { readdirSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const packageRoot = resolve(import.meta.dirname, "..");
const workspaceRoot = join(packageRoot, "tests", "semantic-workspace");
const codemodBinaryPath = join(packageRoot, "..", "..", "target", "release", "codemod");

const LANGUAGE_BY_EXTENSION = {
  ".js": "javascript",
  ".jsx": "jsx",
  ".ts": "typescript",
  ".tsx": "tsx",
};

function collectCaseDirectories(rootDir) {
  const cases = [];

  for (const entry of readdirSync(rootDir, { withFileTypes: true })) {
    const entryPath = join(rootDir, entry.name);

    if (!entry.isDirectory()) {
      continue;
    }

    const codemodPath = join(entryPath, "codemod.ts");
    if (safeIsFile(codemodPath)) {
      cases.push(entryPath);
      continue;
    }

    cases.push(...collectCaseDirectories(entryPath));
  }

  return cases;
}

function safeIsFile(path) {
  try {
    return statSync(path).isFile();
  } catch {
    return false;
  }
}

function getFixtureInput(caseDir) {
  const fixturesDir = join(caseDir, "tests", "fixtures");
  const files = readdirSync(fixturesDir, { withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name.startsWith("input."))
    .map((entry) => entry.name)
    .sort();

  if (files.length !== 1) {
    throw new Error(
      `Expected exactly one input fixture in ${relative(packageRoot, fixturesDir)}, found ${files.length}`,
    );
  }

  const inputName = files[0];
  const extension = inputName.slice(inputName.lastIndexOf("."));
  const language = LANGUAGE_BY_EXTENSION[extension];

  if (!language) {
    throw new Error(`Unsupported fixture extension '${extension}' in ${inputName}`);
  }

  return {
    inputPath: join(fixturesDir, inputName),
    language,
  };
}

function runCase(caseDir) {
  const { inputPath, language } = getFixtureInput(caseDir);
  const codemodPath = join(caseDir, "codemod.ts");
  const relativeCaseDir = relative(workspaceRoot, caseDir);

  if (!safeIsFile(codemodBinaryPath)) {
    throw new Error(
      `Missing release CLI at ${relative(packageRoot, codemodBinaryPath)}. Run 'pnpm build:test-cli' in ${relative(process.cwd(), packageRoot) || "."} first.`,
    );
  }

  const result = spawnSync(
    codemodBinaryPath,
    [
      "jssg",
      "run",
      codemodPath,
      "--language",
      language,
      "--target",
      inputPath,
      "--semantic-workspace",
      workspaceRoot,
      "--allow-dirty",
      "--no-interactive",
    ],
    {
      cwd: packageRoot,
      encoding: "utf8",
    },
  );

  const output = `${result.stdout ?? ""}${result.stderr ?? ""}`.trim();
  const failed = result.status !== 0 || /Failed to execute codemod|Runtime error/.test(output);

  const label = failed ? "FAIL" : "PASS";
  console.log(`[${label}] ${relativeCaseDir}`);
  if (output.length > 0) {
    console.log(output);
  }

  return !failed;
}

function matchesFilter(caseDir, filters) {
  if (filters.length === 0) {
    return true;
  }

  const relativeCaseDir = relative(workspaceRoot, caseDir);
  return filters.some(
    (filter) =>
      relativeCaseDir === filter ||
      relativeCaseDir.startsWith(`${filter}/`) ||
      relativeCaseDir.includes(filter),
  );
}

function main() {
  const filters = process.argv.slice(2);
  const allCases = collectCaseDirectories(workspaceRoot).sort();
  const selectedCases = allCases.filter((caseDir) => matchesFilter(caseDir, filters));

  if (selectedCases.length === 0) {
    const filterSuffix = filters.length > 0 ? ` for filters: ${filters.join(", ")}` : "";
    throw new Error(`No semantic test cases found${filterSuffix}`);
  }

  let allPassed = true;
  for (const caseDir of selectedCases) {
    allPassed = runCase(caseDir) && allPassed;
  }

  if (!allPassed) {
    process.exitCode = 1;
  }
}

main();
