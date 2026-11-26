// Values followed across calls with function summaries, Python. The
// target is app.py; lib.py is in scope only in workspace runs.
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
  const { sources, uses } = dataflow("python", { interprocedural: true });
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
  const argumentEdge = (pattern) =>
    describe(uses.get(named(pattern, "user"))).filter((e) => e.includes(":argument"));
  const carries = (pattern) => {
    const edges = argumentEdge(pattern);
    expect(edges.length === 1 && !edges[0].includes(":stop:"), `${pattern} carries user`, edges);
  };
  const stops = (pattern) => {
    const edges = argumentEdge(pattern);
    expect(edges.length === 1 && edges[0].includes(":stop:"), `${pattern} stops`, edges);
  };
  const reached = async (lookup, start) =>
    describe([...(await crawl(lookup, start)).values()].filter((e) => e));
  const workspace = describe(sources.get(find('identity("b-literal")'))).some((e) =>
    e.startsWith("lib.py:"),
  );

  if (workspace) {
    const fromA = await reached(uses, find("request.args"));
    test("a value passed by one caller does not return to another caller", () => {
      expect(fromA.some((e) => e.endsWith(":a")), "reaches a in from_a", fromA);
      expect(!fromA.some((e) => e.endsWith(":b")), "never reaches b in from_b", fromA);
    });
    test("crawling uses steps into the callee", () => {
      expect(fromA.some((e) => e.startsWith("lib.py:parameter:parameter:value")), "reaches identity's value", fromA);
    });
    const intoLog = await reached(sources, named("log(b)", "b"));
    test("a value's sources do not come from another caller", () => {
      expect(intoLog.some((e) => e.endsWith(':"b-literal"')), "reaches the literal", intoLog);
      expect(!intoLog.some((e) => e.endsWith(":request.args")), "never reaches request.args", intoLog);
    });
    test("an argument the callee doesn't return is marked stop", () => stops("audit(user)"));
    test("an argument the callee returns is not marked stop", () => carries("first(user, 1)"));
    test("only the argument at the returned position carries", () => stops("first(1, user)"));
    test("a keyword argument lands on the parameter of that name", () => carries("by_name(1, b=user)"));
    test("a positional argument before a keyword lands by position", () => stops("by_name(user, b=2)"));
    test("*args takes the extra positional arguments", () => carries("collect(1, user)"));
    test("**kwargs takes the unnamed keyword arguments", () => carries("options(x=user)"));
    test("a constructor's result carries its arguments", () => carries("Repo(user)"));
    test("a method's self is skipped when arguments land", () => carries("repo.get(user)"));
    test("a static method has no self", () => carries("Repo.same(user)"));
    test("a value stored outside the callee does not come back", () => stops("save(user)"));
    test("a call's sources are the arguments that come back and the callee's returned value", () => {
      const from = describe(sources.get(find("first(1, user)")));
      expect(from.some((e) => e.endsWith(":1")), "has 1", from);
      expect(!from.some((e) => e.endsWith(":user")), "not user", from);
      expect(from.some((e) => e.startsWith("lib.py:") && e.endsWith(":return:a")), "has first's returned a", from);
    });
    test("a parameter's sources are every caller's argument", () => {
      const value = uses.get(find("request.args")).find((e) => e.via === "parameter").node;
      const from = describe(sources.get(value));
      expect(from.some((e) => e.endsWith(":request.args")), "has request.args", from);
      expect(from.some((e) => e.endsWith(':"b-literal"')), "has the literal", from);
    });
  } else {
    test("in file scope no step leaves the file", () => {
      const all = [
        ...describe(uses.get(find("request.args"))),
        ...describe(sources.get(find('identity("b-literal")'))),
      ];
      expect(!all.some((e) => e.startsWith("lib.py:")), "stays in app.py", all);
    });
    test("in file scope a call into another file carries every argument", () => carries("audit(user)"));
  }
  test("in either scope a call to a function in the file steps into it", () => {
    const into = describe(uses.get(named("local(user)", "user")));
    expect(into.includes("app.py:parameter:parameter:text"), "steps into local's text", into);
  });
  test("without interprocedural nothing crosses a call", () => {
    const off = dataflow("python", { interprocedural: false });
    const edges = describe(off.uses.get(named("local(user)", "user")));
    expect(!edges.some((e) => e.includes(":parameter:parameter:")), "no parameter step", edges);
  });
  done();
  return null;
}
