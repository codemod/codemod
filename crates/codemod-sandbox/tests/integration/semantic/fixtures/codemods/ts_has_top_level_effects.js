// root.hasTopLevelEffects(): does importing this module run code?
function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

export default function transform(root) {
  // Roots for the other files, reached from App.ts.
  const icon = root.resolveExport("./icons", "Icon").root;
  const barrel = root
    .importers(root.resolveModule("./icons/register"))
    .find((i) => i.kind === "import").root;
  const register = root
    .importers(root.resolveModule("./registry"))
    .find((i) => i.root.filename().endsWith("register.ts")).root;

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
