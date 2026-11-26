// symbols(lang).resolveModule and .specifierFor, and resolveExport on a root,
// with tsconfig `paths` inherited through `extends`.
import { symbols } from "codemod:ast-grep";

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}
const tail = (path) => path && path.split("/").slice(-3).join("/");

export default function transform(root) {
  const { resolveModule, specifierFor } = symbols("typescript");
  expectEqual(tail(resolveModule(root, "@/ui")), "src/ui/index.ts", "alias with *");
  expectEqual(tail(resolveModule(root, "~ui")), "src/ui/index.ts", "exact alias");
  expectEqual(tail(resolveModule(root, "../ui/button")), "src/ui/button.ts", "relative");
  expectEqual(resolveModule(root, "@/nope"), null, "does not resolve");

  // Through the barrel to the declaration, with how the leaf exports it.
  const modal = root.resolveExport("@/ui", "Modal");
  expectEqual(
    [tail(modal.root.filename()), modal.kind, modal.exportName, modal.nameNode.text()],
    ["src/ui/modal.ts", "external", "default", "Modal"],
    "resolveExport through a barrel",
  );
  expectEqual(root.resolveExport("@/ui", "Missing"), null, "not exported");
  const button = root.resolveExport(resolveModule(root, "@/ui"), "Button");
  expectEqual(button.exportName, "Button", "resolveExport from a path");

  // Specifiers for the leaf, in the style of the one being replaced.
  expectEqual(specifierFor(root, modal.root, { like: "@/ui" }), "@/ui/modal", "like an alias");
  expectEqual(specifierFor(root, button.root, { like: "../ui" }), "../ui/button", "like a relative");
  expectEqual(specifierFor(root, button.root, { prefer: "alias" }), "@/ui/button", "prefer alias");
  expectEqual(
    specifierFor(root, button.root, { like: "../ui/index.js" }),
    "../ui/button.js",
    "keeps .js",
  );
  const index = resolveModule(root, "@/ui");
  expectEqual(specifierFor(root, index, { like: "@/ui" }), "@/ui", "index file, like @/");
  expectEqual(specifierFor(root, index), "~ui", "shortest");
  expectEqual(specifierFor(root, index.replace(/index\.ts$/, "missing.ts")), null, "no such file");

  try {
    specifierFor(root, index, { prefer: "short" });
    throw new Error("expected a RangeError");
  } catch (error) {
    if (!String(error.message).includes('prefer must be "relative" or "alias"')) throw error;
  }
  return null;
}
