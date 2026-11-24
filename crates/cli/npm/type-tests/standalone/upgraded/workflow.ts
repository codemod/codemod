// What `codemod workflow upgrade` writes for each kind of jssg transform.
import { parse } from "codemod/ast-grep";
import { forEachFile, shard, step } from "codemod/workflow";
import type { WorkflowContext } from "codemod/workflow";
import removeDependenciesTransform from "./src/remove-dependencies.ts";
import reportTransform, { getSelector as reportSelector } from "./src/report.ts";
import rmdirTransform from "./src/rmdir.ts";
import * as moveStylesTransform from "./src/move-styles.ts";
import { jssgFile, jssgWouldChange } from "codemod-internal:jssg-compat";

type Params = Record<string, string>;

const typescriptFiles = {
  include: ["**/*.js", "**/*.ts"],
  exclude: ["**/node_modules/**"],
  language: "typescript",
};

export default async function main({ params, target }: WorkflowContext<Params>) {
  await step(
    "Rewrite rmdir",
    forEachFile(typescriptFiles, (file) => {
      const out = rmdirTransform(parse(file));
      if (typeof out === "string") file.write(out);
    }),
  );
  await step(
    "Remove dependencies",
    forEachFile(
      {
        include: ["**/package.json"],
        language: "json",
        capabilities: ["child_process", "fs"],
      },
      async (file) => {
        const out = await removeDependenciesTransform(parse(file), {
          params,
          language: "json",
          targetDir: target,
        });
        if (typeof out === "string") file.write(out);
      },
    ),
  );
  const report = await step(
    "Report",
    forEachFile(typescriptFiles, async (file) => {
      const root: Parameters<typeof reportTransform>[0] = parse(file);
      const matches = root.root().findAll(reportSelector({ params }));
      if (matches.length === 0) return;
      const result = await reportTransform(root, {
        params,
        language: "typescript",
        targetDir: target,
        matches,
      });
      if (typeof result === "string") return file.write(result);
      if (typeof result?.edit === "string") file.write(result.edit);
      return result?.data;
    }),
  );
  report.outputs satisfies readonly unknown[];
  await step(
    "Move styles",
    forEachFile({ include: ["**/*.less"], language: "css" }, (file) =>
      jssgFile(moveStylesTransform, file, { params }),
    ),
  );
  await step(
    "Shards",
    shard({
      method: { type: "directory", maxFilesPerShard: 5 },
      files: { language: "css" },
      filter: (file) => jssgWouldChange(moveStylesTransform, file, { language: "css", params }),
    }),
  );
}
