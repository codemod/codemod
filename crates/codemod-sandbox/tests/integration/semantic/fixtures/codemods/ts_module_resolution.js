// resolveModule, resolveExport and specifierFor on a root, with tsconfig
// `paths` inherited through `extends`.
function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}
const tail = (path) => path && path.split("/").slice(-3).join("/");

export default function transform(root) {
  expectEqual(tail(root.resolveModule("@/ui")), "src/ui/index.ts", "alias with *");
  expectEqual(tail(root.resolveModule("~ui")), "src/ui/index.ts", "exact alias");
  expectEqual(tail(root.resolveModule("../ui/button")), "src/ui/button.ts", "relative");
  expectEqual(root.resolveModule("@/nope"), null, "does not resolve");

  // Through the barrel to the declaration, with how the leaf exports it.
  const modal = root.resolveExport("@/ui", "Modal");
  expectEqual(
    [tail(modal.root.filename()), modal.kind, modal.exportName, modal.nameNode.text()],
    ["src/ui/modal.ts", "external", "default", "Modal"],
    "resolveExport through a barrel",
  );
  expectEqual(root.resolveExport("@/ui", "Missing"), null, "not exported");
  const button = root.resolveExport(root.resolveModule("@/ui"), "Button");
  expectEqual(button.exportName, "Button", "resolveExport from a path");

  // Specifiers for the leaf, in the style of the one being replaced.
  expectEqual(root.specifierFor(modal.root, { like: "@/ui" }), "@/ui/modal", "like an alias");
  expectEqual(root.specifierFor(button.root, { like: "../ui" }), "../ui/button", "like a relative");
  expectEqual(root.specifierFor(button.root, { prefer: "alias" }), "@/ui/button", "prefer alias");
  expectEqual(
    root.specifierFor(button.root, { like: "../ui/index.js" }),
    "../ui/button.js",
    "keeps .js",
  );
  const index = root.resolveModule("@/ui");
  expectEqual(root.specifierFor(index, { like: "@/ui" }), "@/ui", "index file, like @/");
  expectEqual(root.specifierFor(index), "~ui", "shortest");
  expectEqual(root.specifierFor("../ui/missing.ts"), null, "no such file");

  try {
    root.specifierFor(index, { prefer: "short" });
    throw new Error("expected a RangeError");
  } catch (error) {
    if (!String(error.message).includes('prefer must be "relative" or "alias"')) throw error;
  }
  return null;
}
