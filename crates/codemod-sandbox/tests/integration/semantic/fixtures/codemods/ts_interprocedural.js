// Data flow across functions, built from the dataflow and callGraph lookups:
// a call's result continues at the callee's returns, a parameter at the
// callers' arguments, and a value that enters a function through a call
// leaves through that same call.
//
// Backward: a crawl of the sources lookup plus a lookup from each call to
// what its callee returns (built from the callees lookup). A parameter
// reached that way continues at the argument of the call it was entered
// through; one reached without entering a call, at every caller's argument.
// Forward: a crawl of the uses lookup plus a lookup from each argument to
// the parameter it binds.
import { callGraph, crawl, dataflow, lookup } from "codemod:ast-grep";
import { excludes, includes, equal, test } from "./spec.js";
export { run as default } from "./spec.js";

const basename = (path) => path.split("/").pop();
const describe = (edges) =>
  edges
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
const find = (root, pattern) => {
  const node = root.root().find({ rule: { pattern } });
  if (!node) throw new Error(pattern + " not found");
  return node;
};
const named = (node) => node.children().filter((n) => n.isNamed());
const argOf = (root, pattern) => named(find(root, pattern).field("arguments"))[0];
const calls = (root) => root.root().findAll({ rule: { kind: "call_expression" } });

// From each call to what its callee returns; `site` is the call entered.
function returnsLookup(root) {
  const edges = [];
  for (const site of calls(root)) {
    for (const { node: callee } of callGraph("typescript").callees.get(site) ?? []) {
      for (const ret of callee.findAll({ rule: { kind: "return_statement" } })) {
        const value = named(ret)[0];
        if (value) {
          edges.push([
            site,
            value,
            { kind: "return", via: "return", name: null, index: null, boundary: false, site },
          ]);
        }
      }
    }
  }
  return lookup(edges);
}

// Every edge reached back from `start`, across functions. A call or a
// parameter the trail went on from is not a boundary.
async function sourcesAcross(root, start) {
  const { sources } = dataflow("typescript");
  const { callers } = callGraph("typescript");
  const returns = returnsLookup(root);
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
        const sites = entered ? [entered] : (callers.get(fn) ?? []).map((c) => c.site);
        for (const call of sites) {
          // The call's own sources say what its argument is.
          const argument = (sources.get(call) ?? []).find(
            (e) => e.via === "argument" && e.index === edge.index,
          );
          if (!argument) continue;
          wentOn.add(node.key());
          if (!found.has(argument.node.key()))
            found.set(argument.node.key(), { ...argument, from: node });
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
}

// Every edge reached forward from req.body.name, into callees' parameters.
async function usesOfName(root) {
  const intoParameters = [];
  for (const site of calls(root)) {
    for (const { node: callee } of callGraph("typescript").callees.get(site) ?? []) {
      const parameters = named(callee.field("parameters"));
      named(site.field("arguments")).forEach((argument, i) => {
        const parameter = parameters[i];
        if (!parameter) return;
        const binding = parameter.field("pattern") ?? parameter;
        intoParameters.push([
          argument,
          binding,
          { kind: "parameter", via: "parameter", name: binding.text(), index: i },
        ]);
      });
    }
  }
  const reached = await crawl(
    [dataflow("typescript").uses, lookup(intoParameters)],
    find(root, "req.body.name"),
  );
  return [...reached.values()].filter((edge) => edge);
}

// log(other): other is normalize("literal"), which returns value.trim() in
// lib.ts, where value is "literal".
test("a call's result continues at what the callee returns", async (root) => {
  const other = describe(await sourcesAcross(root, argOf(root, "log($A)")));
  includes(other, 'app.ts:call:normalize("literal")');
  includes(other, "lib.ts:return:value.trim()");
});

test("a parameter entered through a call continues at that call's argument only", async (root) => {
  const other = describe(await sourcesAcross(root, argOf(root, "log($A)")));
  includes(other, "lib.ts:parameter:value");
  includes(other, 'app.ts:literal:"literal"');
  excludes(other, "app.ts:member:req.body.name");
});

test("the sources lookup alone stays in its function: the call is a boundary", async (root) => {
  const shallow = describe(
    [...(await crawl(dataflow("typescript").sources, argOf(root, "log($A)"))).values()].filter(
      (e) => e,
    ),
  );
  includes(shallow, 'app.ts:call:normalize("literal")!');
  equal(
    shallow.filter((s) => s.startsWith("lib.ts:")),
    [],
  );
});

test("a parameter with callers continues at every call site's argument", async (root) => {
  const x = describe(await sourcesAcross(root, find(root, "return x").child(1)));
  includes(x, "app.ts:parameter:x");
  includes(x, "app.ts:literal:5");
  includes(x, "app.ts:member:req.body.name");
});

test("uses across functions go into the callees' parameters", async (root) => {
  const to = describe(await usesOfName(root));
  includes(to, "lib.ts:parameter:value");
  includes(to, "lib.ts:parameter:sql");
});

test("a value leaves a function only through the call it entered", async (root) => {
  const to = describe(await usesOfName(root));
  includes(to, "app.ts:variable:name");
  excludes(to, "app.ts:variable:other");
});

test("edge.from crosses files", async (root) => {
  const sql = (await usesOfName(root)).find((e) => e.kind === "parameter" && e.name === "sql");
  equal(basename(sql.from.getRoot().filename()), "app.ts");
});
