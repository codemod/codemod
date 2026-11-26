// root.hasTopLevelEffects(): does importing this module run code?
import { symbols } from "codemod:ast-grep";

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

export default function transform(root) {
  // Roots for the other files, reached from App.ts through references.
  const { references } = symbols("typescript");
  const rootReferring = (root, name, file) =>
    references
      .get(root.root().find({ rule: { kind: "identifier", regex: `^${name}$` } }))
      .map((e) => e.node.getRoot())
      .find((r) => r.filename().endsWith(file));
  const icon = root.resolveExport("./icons", "Icon").root;
  const barrel = rootReferring(icon, "Icon", "icons/index.ts");
  const register = rootReferring(root.resolveExport("./registry", "registry").root, "registry", "register.ts");

  expectEqual(
    {
      app: root.hasTopLevelEffects(),
      icon: icon.hasTopLevelEffects(),
      barrel: barrel.hasTopLevelEffects(),
      register: register.hasTopLevelEffects(),
    },
    {
      app: true, // Icon("check")
      icon: false, // declares only
      barrel: true, // import "./register"
      register: true, // registry.set(...)
    },
    "top-level effects",
  );
  return null;
}
