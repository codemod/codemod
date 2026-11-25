// Without a semantic provider the data-flow and call-graph queries throw:
// an empty answer would read as "no callers" or "no sources".
function expectThrow(run, needle, label) {
  try {
    run();
  } catch (error) {
    if (!String(error.message).includes(needle)) {
      throw new Error(label + ": unexpected message: " + error.message);
    }
    return;
  }
  throw new Error(label + ": expected an error");
}

export default function transform(root) {
  const node = root.root().find({ rule: { kind: "identifier" } });
  for (const method of ["flowsFrom", "flowsTo", "callees", "callers", "mutations"]) {
    expectThrow(() => node[method](), method + "() needs semantic analysis", method);
  }
  expectThrow(() => node.callers(), 'semanticAnalysis: "file"', "says what to set");
  return null;
}
