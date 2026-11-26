// Across functions, built from the dataflow and callGraph lookups: a call's
// result continues at the callee's returns, a parameter at the callers'
// arguments, and a value that enters a function through a call leaves
// through that same call.
//
// Backward, a crawl of the sources lookup plus a lookup from each call to
// what its callee returns (built from the callees lookup) goes into the
// callees; a parameter reached that way continues at the argument of the
// call it was entered through, and one reached without entering a call at
// every caller's argument (the callers lookup). Forward, a crawl of the uses
// lookup plus a lookup from each argument to the parameter it binds goes
// into the callees; what a call returns comes from its own arguments, so the
// value leaves through that same call.
import { callGraph, crawl, dataflow, lookup } from "codemod:ast-grep";

function basename(path) {
  return path.split("/").pop();
}

function describe(edges) {
  return edges
    .map(
      (e) =>
        basename(e.node.getRoot().filename()) +
        ":" +
        e.kind +
        ":" +
        e.node.text() +
        (e.boundary ? "!" : ""),
    )
    .sort();
}

function expectHas(list, entry, label) {
  if (!list.includes(entry)) {
    throw new Error(label + ": expected " + entry + " in " + JSON.stringify(list));
  }
}

function expectLacks(list, entry, label) {
  if (list.includes(entry)) {
    throw new Error(label + ": did not expect " + entry + " in " + JSON.stringify(list));
  }
}

export default async function transform(root) {
  const find = (pattern) => {
    const node = root.root().find({ rule: { pattern } });
    if (!node) throw new Error(pattern + " not found");
    return node;
  };
  const argOf = (pattern) =>
    find(pattern)
      .field("arguments")
      .children()
      .filter((n) => n.isNamed())[0];
  const { sources, uses } = dataflow("typescript");
  const { callees, callers } = callGraph("typescript");
  const named = (node) => node.children().filter((n) => n.isNamed());
  const sites = root.root().findAll({ rule: { kind: "call_expression" } });

  // From each call to what its callee returns. `site` says which call the
  // callee was entered through.
  const intoReturns = [];
  for (const site of sites) {
    for (const { node: callee } of callees.get(site) ?? []) {
      for (const ret of callee.findAll({ rule: { kind: "return_statement" } })) {
        const value = named(ret)[0];
        if (!value) continue;
        intoReturns.push([
          site,
          value,
          { kind: "return", via: "return", name: null, index: null, boundary: false, site },
        ]);
      }
    }
  }
  const returns = lookup(intoReturns);

  // Every edge reached back from `start`, across functions. A call or a
  // parameter the trail went on from is not a boundary.
  const sourcesAcross = async (start) => {
    const found = new Map();
    const wentOn = new Set();
    const crawled = new Set();
    for (let starts = [start]; starts.length > 0; ) {
      const next = [];
      for (const from of starts) {
        if (crawled.has(from.key())) continue;
        crawled.add(from.key());
        const reached = await crawl([sources, returns], from);
        for (const [node, edge] of reached) {
          if (!edge) continue;
          if (!found.has(node.key())) found.set(node.key(), edge);
          if (edge.site) wentOn.add(edge.from.key());
          if (edge.kind !== "parameter") continue;
          // The call this parameter's function was entered through, if any.
          let entered = null;
          for (let e = edge; e && !entered; e = reached.get(e.from)) entered = e.site ?? null;
          const fn = node.parent().parent();
          const calls = entered ? [entered] : (callers.get(fn) ?? []).map((c) => c.site);
          for (const call of calls) {
            // The call's own sources say what its argument is.
            const argument = (sources.get(call) ?? []).find(
              (e) => e.via === "argument" && e.index === edge.index,
            );
            if (!argument) continue;
            wentOn.add(node.key());
            if (!found.has(argument.node.key())) {
              found.set(argument.node.key(), { ...argument, from: node });
            }
            next.push(argument.node);
          }
        }
      }
      starts = next;
    }
    return [...found.values()].map((e) => ({
      ...e,
      boundary: e.boundary && !wentOn.has(e.node.key()),
    }));
  };

  // log(other): other is normalize("literal"), which returns value.trim()
  // in lib.ts, where value is "literal", and only that call's argument.
  const other = describe(await sourcesAcross(argOf("log($A)")));
  expectHas(other, 'app.ts:call:normalize("literal")', "log(other)");
  expectHas(other, "lib.ts:return:value.trim()", "log(other)");
  expectHas(other, "lib.ts:parameter:value", "log(other)");
  expectHas(other, 'app.ts:literal:"literal"', "log(other)");
  expectLacks(other, "app.ts:member:req.body.name", "log(other)");
  // The sources lookup alone stays in the function: the call is a boundary
  // and lib.ts is not entered.
  const shallow = describe([...(await crawl(sources, argOf("log($A)"))).values()].filter((e) => e));
  expectHas(shallow, 'app.ts:call:normalize("literal")!', "log(other) intraprocedural");
  if (shallow.some((s) => s.startsWith("lib.ts:"))) {
    throw new Error("a crawl of the sources lookup left the file: " + JSON.stringify(shallow));
  }

  // A parameter with callers is not a boundary: its sources are the
  // arguments at every call site.
  const x = describe(await sourcesAcross(find("return x").child(1)));
  expectHas(x, "app.ts:parameter:x", "x");
  expectHas(x, "app.ts:literal:5", "x");
  expectHas(x, "app.ts:member:req.body.name", "x");

  // req.body.name goes into normalize's value, back out to its own call
  // only, then into query's sql.
  const intoParameters = [];
  for (const site of sites) {
    for (const { node: callee } of callees.get(site) ?? []) {
      const parameters = named(callee.field("parameters"));
      named(site.field("arguments")).forEach((argument, i) => {
        const parameter = parameters[i];
        if (!parameter) return;
        const binding = parameter.field("pattern") ?? parameter;
        const name = binding.text();
        intoParameters.push([argument, binding, { kind: "parameter", via: "parameter", name, index: i }]);
      });
    }
  }
  const reached = await crawl([uses, lookup(intoParameters)], find("req.body.name"));
  const body = [...reached.values()].filter((edge) => edge);
  const to = describe(body);
  expectHas(to, "lib.ts:parameter:value", "req.body.name");
  expectHas(to, "lib.ts:parameter:sql", "req.body.name");
  expectHas(to, "app.ts:variable:name", "req.body.name");
  expectLacks(to, "app.ts:variable:other", "req.body.name");
  // edge.from crosses files: lib.ts's sql was reached from the argument
  // "select " + name in app.ts.
  const sql = body.find((e) => e.kind === "parameter" && e.name === "sql");
  if (!sql.from || basename(sql.from.getRoot().filename()) !== "app.ts") {
    throw new Error("sql.from should be a node in app.ts");
  }
  return null;
}
