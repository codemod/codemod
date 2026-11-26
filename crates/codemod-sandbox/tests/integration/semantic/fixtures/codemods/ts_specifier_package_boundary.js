// specifierFor across workspace packages: only what the target package makes
// public (its exports map, or its bare name without one), never a relative
// path into its files.
import { symbols } from "codemod:ast-grep";

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

export default function transform(root) {
  const { resolveModule, specifierFor, references } = symbols("typescript");
  const leaf = (module, name) => root.resolveExport(module, name).root;
  const like = { like: "@acme/ui" };
  expectEqual(
    specifierFor(root, leaf("@acme/ui", "Button"), like),
    "@acme/ui/button",
    "exports subpath with conditions",
  );
  expectEqual(
    specifierFor(root, leaf("@acme/ui", "Check"), like),
    "@acme/ui/icons/Check",
    "exports pattern",
  );
  expectEqual(
    specifierFor(root, leaf("@acme/ui", "Modal"), like),
    null,
    "not exported: no specifier",
  );
  expectEqual(specifierFor(root, resolveModule(root, "@acme/ui"), like), "@acme/ui", "package entry");
  expectEqual(
    specifierFor(root, leaf("@acme/lib", "format"), { like: "@acme/lib" }),
    null,
    "no exports: deep files are private",
  );
  expectEqual(
    specifierFor(root, resolveModule(root, "@acme/lib")),
    "@acme/lib",
    "no exports: the entry",
  );

  // Which files are public, seen from the files themselves.
  expectEqual(leaf("@acme/ui", "Button").publicSpecifiers(), ["@acme/ui/button"], "public subpath");
  expectEqual(leaf("@acme/ui", "Modal").publicSpecifiers(), [], "private file");
  expectEqual(
    leaf("@acme/lib", "format").publicSpecifiers(),
    [],
    "no exports: deep file is private",
  );
  expectEqual(root.publicSpecifiers(), [], "the app file");
  // The package entry, as the file that re-exports Button.
  const button = leaf("@acme/ui", "Button");
  const entry = references
    .get(button.root().find({ rule: { kind: "identifier", regex: "^Button$" } }))
    .map((e) => e.node.getRoot())
    .find((r) => r.filename().endsWith("ui/src/index.ts"));
  expectEqual(entry.publicSpecifiers(), ["@acme/ui"], "package entry");

  // Inside one package, relative paths as before.
  if (!resolveModule(root, "@acme/ui") || !button) throw new Error("did not resolve");
  return null;
}
