// A JSSG definition in a module the workflow imports: the loader must split
// every module in the graph, not only the entry.
import { jssg } from "../../../src/index.ts";
import { migrateText } from "../jssg/helpers.ts";

export const migrate = jssg({
  name: "migrate",
  language: "typescript",
  include: ["**/*.ts"],
  transform(root) {
    return migrateText(root.root().text());
  },
});
