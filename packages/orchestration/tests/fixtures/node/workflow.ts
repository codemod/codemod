// A shell + jssg workflow run end to end on both the node and quickjs runtimes.
import { jssg, sequence, shell } from "../../../src/index.ts";
import { migrateText, posixPath } from "../jssg/helpers.ts";

const seed = shell({
  name: "seed",
  command: "mkdir -p src && printf 'oldApi(3);\\n' > src/seeded.ts && echo seeded",
});

const migrate = jssg({
  name: "migrate",
  language: "typescript",
  include: ["src/**/*.ts"],
  transform(root) {
    const text = root.root().text();
    if (!text.includes("oldApi")) return null;
    return { content: migrateText(text), output: posixPath(root.relativeFilename()) };
  },
});

export default sequence(seed(), migrate());
