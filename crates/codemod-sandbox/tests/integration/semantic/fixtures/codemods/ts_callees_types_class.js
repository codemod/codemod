// x.save() where x: UserRepo, a class: the call graph follows the type.
// Needs a TypeScript 7 checker (CODEMOD_TS_API).
function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

function basename(path) {
  return path.split("/").pop();
}

export default function transform(root) {
  const register = root.root().find({ rule: { kind: "function_declaration" } });
  const call = register.find({ rule: { pattern: "repo.save($$$)" } });

  // The call resolves to the class method, marked as found through types.
  expectEqual(
    call.callees().map((c) => [c.name, basename(c.root.filename()), c.via, c.resolved]),
    [["save", "repo.ts", "type", true]],
    "callees of repo.save()",
  );
  // And the walk continues through it.
  expectEqual(
    register
      .callees({ transitive: true })
      .filter((c) => c.name === "audit")
      .map((c) => [c.name, c.via, c.depth]),
    [["audit", "direct", 2]],
    "transitive callees through the typed edge",
  );
  // Callers of the method see the typed call too.
  const save = call.callees()[0].node;
  expectEqual(
    save.callers().map((c) => [c.name, c.via]),
    [["register", "type"]],
    "callers of UserRepo.save",
  );
  // stopAt can leave typed edges out of a walk.
  expectEqual(
    register.callees({ transitive: true, stopAt: ["type"] }).filter((c) => c.name === "audit"),
    [],
    "stopAt type",
  );

  // typeInfo(): the checker's type for an expression.
  const repo = call.find({ rule: { pattern: "repo" } });
  expectEqual(repo.typeInfo(), "UserRepo", "type of repo");
  expectEqual(call.typeInfo(), "void", "type of repo.save(...)");
  const arg = call.find({ rule: { kind: "object" } });
  expectEqual(arg.typeInfo(), "{ id: string; }", "type of the argument");
  return null;
}
