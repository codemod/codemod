/**
 * React 19 `act` import migration on one repository: branch, count, rewrite,
 * count again, check the diff, commit. A real migration shape: transform,
 * then verify before anything is committed.
 *
 *   codemod workflow run -w workflow.ts --target ~/repos/backstage --input '{"base":"origin/master"}'
 */
import { dynamic, guard, jssg, shell } from "@codemod.com/orchestration";
import { isActOnly, rewrite, testUtilsImports } from "./act.ts";

const BRANCH = "codemod/react-19-act-import";
type Input = { base: string };
const isInput = (v: unknown): v is Input => typeof (v as Input)?.base === "string";

const branch = shell({
  name: "branch",
  input: guard("{ base }", isInput),
  command: ({ base }) => `git checkout -q -f -B ${BRANCH} '${base}' && git rev-parse --short HEAD`,
});

const detect = jssg({
  name: "find-test-utils-imports",
  language: "tsx",
  include: ["**/*.tsx", "**/*.ts", "**/*.jsx", "**/*.js"],
  exclude: ["**/node_modules/**", "**/dist/**"],
  selector: { rule: { pattern: "'react-dom/test-utils'" } },
  transform(root) {
    const imports = testUtilsImports(root);
    if (imports.length === 0) return null;
    return {
      output: {
        file: root.relativeFilename(),
        mechanical: imports.filter(isActOnly).length,
        manual: imports.filter((i) => !isActOnly(i)).map((i) => i.text()),
      },
    };
  },
});

const migrate = jssg({
  name: "replace-act-import",
  language: "tsx",
  include: ["**/*.tsx", "**/*.ts", "**/*.jsx", "**/*.js"],
  exclude: ["**/node_modules/**", "**/dist/**"],
  selector: { rule: { pattern: "'react-dom/test-utils'" } },
  transform(root) {
    const edits = testUtilsImports(root).filter(isActOnly).map(rewrite);
    if (edits.length === 0) return null;
    return {
      content: root.root().commitEdits(edits),
      output: { file: root.relativeFilename(), edits: edits.length },
    };
  },
});

const diff = shell({
  name: "verify-diff",
  // --check fails on whitespace damage; numstat is what the results view shows.
  command: `git diff --check && git diff --numstat`,
});

const commit = shell({
  name: "commit",
  command: `git -c user.name=codemod -c user.email=codemod@localhost commit -q -am "Move act imports from react-dom/test-utils to react" && git rev-parse --short HEAD`,
});

type Found = { file: string; mechanical: number; manual: string[] };
const count = (rows: Found[]) => rows.reduce((n, r) => n + r.mechanical, 0);

export default dynamic(async (input: Input) => {
  const base = (await branch({ input })).stdout.trim();
  const before = (await detect({ id: "detect:before" })) as Found[];
  const changed = (await migrate()) as { file: string; edits: number }[];
  const after = (await detect({ id: "detect:after" })) as Found[];
  if (count(after) > 0)
    throw new Error(`${count(after)} act imports still come from react-dom/test-utils`);
  const numstat = (await diff()).stdout
    .split("\n")
    .filter(Boolean)
    .map((line) => {
      const [added, removed, file] = line.split("\t");
      return { file: file!, added: Number(added), removed: Number(removed) };
    });
  const sha = changed.length > 0 ? (await commit()).stdout.trim() : null;
  return {
    kind: "react-19-act-migration",
    branch: BRANCH,
    base,
    commit: sha,
    before: {
      files: before.length,
      mechanical: count(before),
      manual: before.flatMap((r) => r.manual.map((text) => ({ file: r.file, text }))),
    },
    after: {
      files: after.length,
      mechanical: count(after),
      manual: after.flatMap((r) => r.manual.map((text) => ({ file: r.file, text }))),
    },
    changed,
    diff: numstat,
  };
});
