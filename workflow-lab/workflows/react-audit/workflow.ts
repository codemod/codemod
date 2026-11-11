/**
 * React audit: nine detection codemods from the jssg React/Next.js audit,
 * fanned out over every git repository in --target. Nothing is written.
 *
 *   codemod workflow run -w workflow.ts --target ~/repos
 */
import { dynamic, guard, jssg, shell } from "@codemod.com/orchestration";
import { detectAll, type Finding } from "./rules/index.ts";

interface RepoInfo {
  commit: string;
  sourceFiles: number;
}

const listRepos = shell({
  name: "list-repos",
  command: `for d in */; do [ -e "$d.git" ] && printf '%s\\n' "\${d%/}"; done; true`,
});

const repoInfo = shell({
  name: "repo-info",
  input: guard(
    "{ repo }",
    (v): v is { repo: string } => typeof (v as { repo?: unknown })?.repo === "string",
  ),
  // shell() has no cwd: every per-repo command has to cd itself.
  command: ({ repo }) =>
    `cd '${repo}' && printf '{"commit":"%s","sourceFiles":%s}' "$(git rev-parse --short HEAD)" "$(git ls-files '*.ts' '*.tsx' '*.js' '*.jsx' | wc -l | tr -d ' ')"`,
  output: guard("RepoInfo", (v): v is RepoInfo => typeof (v as RepoInfo)?.commit === "string"),
});

const audit = jssg({
  name: "react-audit",
  language: "tsx",
  include: ["**/*.tsx", "**/*.jsx", "**/*.ts", "**/*.js"],
  exclude: [
    "**/node_modules/**",
    "**/dist/**",
    "**/*.d.ts",
    "**/*.test.*",
    "**/*.spec.*",
    "**/*.stories.*",
    "**/__tests__/**",
    "**/e2e/**",
    "**/fixtures/**",
  ],
  async transform(root, options) {
    const findings = await detectAll(root, options);
    return findings.length === 0 ? null : { output: { file: root.relativeFilename(), findings } };
  },
});

export default dynamic(async () => {
  const { stdout } = await listRepos();
  const repos = stdout.split("\n").filter(Boolean);

  const results = await Promise.all(
    repos.map(async (repo) => {
      const [info, files] = await Promise.all([
        repoInfo({ id: `info:${repo}`, input: { repo } }),
        audit({ id: `audit:${repo}`, target: { root: repo } }) as Promise<
          { file: string; findings: Finding[] }[]
        >,
      ]);
      const byRule: Record<string, number> = {};
      const rows = files.flatMap(({ file, findings }) =>
        findings.map((f) => {
          byRule[f.rule] = (byRule[f.rule] ?? 0) + 1;
          return { file, line: f.line, rule: f.rule, dims: f.dims };
        }),
      );
      return {
        repo,
        ...info,
        filesWithFindings: files.length,
        total: rows.length,
        byRule,
        findings: rows,
      };
    }),
  );

  return { kind: "react-audit", repos: results, total: results.reduce((n, r) => n + r.total, 0) };
});
