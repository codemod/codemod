// Values followed across calls with function summaries, TypeScript. The
// target is app.ts; lib.ts is in scope only in workspace runs.
import { crawl, dataflow } from "codemod:ast-grep";

// Named one-rule cases, all run, failures reported together.
function cases() {
  const failed = [];
  const test = (name, body) => {
    try {
      body();
    } catch (error) {
      failed.push(`${name}: ${error.message}`);
    }
  };
  const done = () => {
    if (failed.length) throw new Error("\n" + failed.join("\n"));
  };
  return { test, done };
}

const basename = (path) => path.split("/").pop();

// "file:kind:via[:stop]:text" for each edge.
const describe = (edges) =>
  (edges ?? []).map(
    (e) =>
      `${basename(e.node.getRoot().filename())}:${e.kind}:${e.via}${e.stop ? ":stop" : ""}:${e.node.text()}`,
  );

function expect(condition, what, seen) {
  if (!condition) throw new Error(`${what}; got ${JSON.stringify(seen)}`);
}

export default async function transform(root) {
  const { test, done } = cases();
  const { sources, uses } = dataflow("typescript", { interprocedural: true });
  const find = (pattern) => {
    const node = root.root().find({ rule: { pattern } });
    if (!node) throw new Error(`${pattern} not found`);
    return node;
  };
  const named = (pattern, name) => {
    const node = find(pattern).find({ rule: { kind: "identifier", regex: `^${name}$` } });
    if (!node) throw new Error(`${name} not found in ${pattern}`);
    return node;
  };
  const argumentEdge = (pattern, name) =>
    describe(uses.get(named(pattern, name))).filter((e) => e.includes(":argument"));
  const reached = async (lookup, start) =>
    describe([...(await crawl(lookup, start)).values()].filter((e) => e));
  const workspace = describe(sources.get(find('identity("b-literal")'))).some((e) =>
    e.startsWith("lib.ts:"),
  );

  if (workspace) {
    const fromA = await reached(uses, find("req.body.a"));
    test("a value passed by one caller does not return to another caller", () => {
      expect(fromA.some((e) => e.endsWith(":a")), "reaches a in fromA", fromA);
      expect(!fromA.some((e) => e.endsWith(":b")), "never reaches b in fromB", fromA);
    });
    test("crawling uses steps into the callee", () => {
      expect(fromA.some((e) => e.startsWith("lib.ts:parameter:parameter:value")), "reaches identity's value", fromA);
    });
    const intoLog = await reached(sources, named("log(b)", "b"));
    test("a value's sources do not come from another caller", () => {
      expect(intoLog.some((e) => e.endsWith(':"b-literal"')), "reaches the literal", intoLog);
      expect(!intoLog.some((e) => e.endsWith(":req.body.a")), "never reaches req.body.a", intoLog);
    });
    test("an argument the callee doesn't return is marked stop", () => {
      const edges = argumentEdge("audit(user)", "user");
      expect(edges.length === 1 && edges[0].includes(":stop:"), "audit(user) stops", edges);
    });
    test("an argument the callee returns is not marked stop", () => {
      const edges = argumentEdge("first(user, 1)", "user");
      expect(edges.length === 1 && !edges[0].includes(":stop:"), "first(user, 1) carries user", edges);
    });
    test("only the argument at the returned position carries", () => {
      const edges = argumentEdge("first(1, user)", "user");
      expect(edges.length === 1 && edges[0].includes(":stop:"), "first(1, user) stops", edges);
    });
    test("a value stored in a local object that is returned comes back", () => {
      const edges = argumentEdge("wrap(user)", "user");
      expect(!edges[0].includes(":stop:"), "wrap(user) carries user", edges);
    });
    test("a value stored outside the callee does not come back", () => {
      const edges = argumentEdge("save(user)", "user");
      expect(edges[0].includes(":stop:"), "save(user) stops", edges);
    });
    test("a closure that returns the parameter carries it", () => {
      const edges = argumentEdge("later(user)", "user");
      expect(!edges[0].includes(":stop:"), "later(user) carries user", edges);
    });
    test("a rest parameter takes the extra arguments", () => {
      const edges = argumentEdge("rest(1, user)", "user");
      expect(!edges[0].includes(":stop:"), "rest(1, user) carries user", edges);
      const into = describe(uses.get(named("rest(1, user)", "user")));
      expect(into.some((e) => e.startsWith("lib.ts:parameter:parameter:") && e.endsWith("values")), "steps into values", into);
    });
    test("a call's sources are the arguments that come back and the callee's returned value", () => {
      const from = describe(sources.get(find("first(1, user)")));
      expect(from.some((e) => e.endsWith(":1")), "has 1", from);
      expect(!from.some((e) => e.endsWith(":user")), "not user", from);
      expect(from.some((e) => e.startsWith("lib.ts:") && e.endsWith(":return:a")), "has first's returned a", from);
    });
    test("a parameter's sources are every caller's argument", () => {
      const value = uses.get(find("req.body.a")).find((e) => e.via === "parameter").node;
      const from = describe(sources.get(value));
      expect(from.some((e) => e.endsWith(":req.body.a")), "has req.body.a", from);
      expect(from.some((e) => e.endsWith(':"b-literal"')), "has the literal", from);
    });
  } else {
    test("in file scope no step leaves the file", () => {
      const all = [
        ...describe(uses.get(find("req.body.a"))),
        ...describe(sources.get(find('identity("b-literal")'))),
      ];
      expect(!all.some((e) => e.startsWith("lib.ts:")), "stays in app.ts", all);
    });
    test("in file scope a call into another file carries every argument", () => {
      const edges = argumentEdge("audit(user)", "user");
      expect(!edges[0].includes(":stop:"), "audit(user) carries user", edges);
    });
  }
  test("in either scope a call to a function in the file steps into it", () => {
    const into = describe(uses.get(named("local(user)", "user")));
    expect(into.includes("app.ts:parameter:parameter:text"), "steps into local's text", into);
  });
  test("without interprocedural nothing crosses a call", () => {
    const off = dataflow("typescript", { interprocedural: false });
    const edges = describe(off.uses.get(named("local(user)", "user")));
    expect(!edges.some((e) => e.includes(":parameter:parameter:")), "no parameter step", edges);
  });
  done();
  return null;
}
