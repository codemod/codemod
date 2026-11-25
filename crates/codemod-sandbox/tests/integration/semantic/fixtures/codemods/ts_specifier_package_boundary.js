// specifierFor across workspace packages: only what the target package makes
// public (its exports map, or its bare name without one), never a relative
// path into its files.
function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

export default function transform(root) {
  const leaf = (module, name) => root.resolveExport(module, name).root;
  const like = { like: "@acme/ui" };
  expectEqual(
    root.specifierFor(leaf("@acme/ui", "Button"), like),
    "@acme/ui/button",
    "exports subpath with conditions",
  );
  expectEqual(
    root.specifierFor(leaf("@acme/ui", "Check"), like),
    "@acme/ui/icons/Check",
    "exports pattern",
  );
  expectEqual(
    root.specifierFor(leaf("@acme/ui", "Modal"), like),
    null,
    "not exported: no specifier",
  );
  expectEqual(root.specifierFor(root.resolveModule("@acme/ui"), like), "@acme/ui", "package entry");
  expectEqual(
    root.specifierFor(leaf("@acme/lib", "format"), { like: "@acme/lib" }),
    null,
    "no exports: deep files are private",
  );
  expectEqual(
    root.specifierFor(root.resolveModule("@acme/lib")),
    "@acme/lib",
    "no exports: the entry",
  );

  // Inside one package, relative paths as before.
  const index = root.resolveModule("@acme/ui");
  const button = leaf("@acme/ui", "Button");
  if (!index || !button) throw new Error("did not resolve");
  return null;
}
