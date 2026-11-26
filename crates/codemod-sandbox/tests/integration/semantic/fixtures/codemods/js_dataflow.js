// Data flow and the call graph in JavaScript: the sources and uses lookups,
// crawls over them, mutations(), the callees and callers lookups, node
// identity, and the errors a mistaken query throws.
import { callGraph, crawl, dataflow, parse, symbols } from "codemod:ast-grep";
import { equal, excludes, includes, onFile, test, throws } from "./spec.js";
export { run as default } from "./spec.js";

const find = (root, pattern) => {
  const node = root.root().find({ rule: { pattern } });
  if (!node) throw new Error(pattern + " not found");
  return node;
};
const argsOf = (root, pattern) =>
  find(root, pattern)
    .field("arguments")
    .children()
    .filter((n) => n.isNamed());
const argOf = (root, pattern) => argsOf(root, pattern)[0];

// Each edge as kind:text, with ! for a boundary, sorted.
const describe = (edges) =>
  edges.map((e) => e.kind + ":" + e.node.text() + (e.boundary ? "!" : "")).sort();
// Every edge a crawl of the sources lookup reached (the start maps to null).
const sourcesOf = async (node, options) =>
  [...(await crawl(dataflow("javascript").sources, node, options)).values()].filter((e) => e);

// A function's name: its own, or the name it is bound to.
function nameOf(fn) {
  const own = fn.field("name");
  if (own) return own.text();
  const parent = fn.parent();
  const bound = parent && (parent.field("name") || parent.field("key"));
  return bound ? bound.text() : "<anonymous>";
}
const basename = (path) => path.split("/").pop();
// Hops back to the start along `from`.
function depthOf(reached, node) {
  let depth = 0;
  for (let edge = reached.get(node); edge; edge = reached.get(edge.from)) depth++;
  return depth;
}
// Each reached node as name:via:depth, sorted.
const walk = (reached) =>
  [...reached]
    .filter(([, edge]) => edge)
    .map(([node, edge]) => nameOf(node) + ":" + edge.via + ":" + depthOf(reached, node))
    .sort();

onFile("server.js", () => {
  const handler = (root) => root.root().find({ rule: { kind: "function_declaration" } });
  // query("select … " + id, limit): limit is assigned 10, then Number(req.query.limit).
  const limit = (root) => argsOf(root, "query($$$ARGS)")[1];
  const sql = (root) => argsOf(root, "query($$$ARGS)")[0];

  test("sources of a reassigned variable include every value assigned to it", async (root) => {
    const found = describe(await sourcesOf(limit(root)));
    includes(found, "variable:limit");
    includes(found, "literal:10");
    includes(found, "call:Number(req.query.limit)!");
  });

  test("sources go on into a call's arguments, to the parameter", async (root) => {
    const found = describe(await sourcesOf(limit(root)));
    includes(found, "member:req.query.limit");
    includes(found, "parameter:req!");
  });

  test("sources leave out values the argument never came from", async (root) => {
    equal(
      (await sourcesOf(limit(root))).filter((e) => e.name === "res"),
      [],
    );
    excludes(describe(await sourcesOf(sql(root))).join(), "req.body");
  });

  test("the nearest source comes first, reached from the start", async (root) => {
    const [first] = await sourcesOf(limit(root));
    equal([first.kind, first.via, first.from.equals(limit(root))], ["variable", "read", true]);
  });

  test("a parameter edge names the parameter and its index", async (root) => {
    const param = (await sourcesOf(limit(root))).find((e) => e.kind === "parameter");
    equal([param.name, param.index], ["req", 0]);
  });

  test("stopAt a call ends the trail there", async (root) => {
    const calls = (await sourcesOf(limit(root)))
      .filter((e) => e.kind === "call")
      .map((e) => e.node);
    const found = describe(await sourcesOf(limit(root), { stopAt: calls }));
    includes(found, "call:Number(req.query.limit)!");
    excludes(found, "parameter:req!");
  });

  test("sources of a string built by concatenation reach the parameter", async (root) => {
    const found = describe(await sourcesOf(sql(root)));
    includes(found, "member:req.params.id");
    includes(found, "parameter:req!");
    excludes(found.join(), "call:");
  });

  // Every call receiving a value derived from req.body: the argument edges
  // leaving the nodes a crawl of uses walked through.
  async function callsReceiving(root, { throughCalls }) {
    const { uses } = dataflow("javascript");
    const body = find(root, "req.body");
    let reached = await crawl(uses, body);
    let stops = [];
    if (!throughCalls) {
      stops = [...reached.values()].filter((e) => e?.via === "argument").map((e) => e.node);
      reached = await crawl(uses, body, { stopAt: stops });
    }
    const calls = new Set();
    for (const node of reached.keys()) {
      if (stops.some((stop) => stop.equals(node))) continue;
      for (const edge of uses.get(node) ?? []) {
        if (edge.via === "argument") calls.add(edge.node.text() + "#" + edge.index);
      }
    }
    return [...calls].sort();
  }

  test("uses pass a value on through what a call returns", async (root) => {
    equal(await callsReceiving(root, { throughCalls: true }), [
      "createUser(label, email)#0",
      "createUser(label, email)#1",
      "log(user)#0",
      "res.json(user)#0",
    ]);
  });

  test("stopAt the receiving calls leaves only the direct receivers", async (root) => {
    equal(await callsReceiving(root, { throughCalls: false }), [
      "createUser(label, email)#0",
      "createUser(label, email)#1",
    ]);
  });

  test("callees are the workspace functions called, not built-ins", (root) => {
    equal(
      callGraph("javascript")
        .callees.get(handler(root))
        .map((e) => nameOf(e.node) + "@" + basename(e.node.getRoot().filename()))
        .sort(),
      ["createUser@users.js", "query@db.js"],
    );
  });

  test("a crawl of callees reaches every function called, with how and how deep", async (root) => {
    const reached = await crawl(callGraph("javascript").callees, handler(root));
    equal(walk(reached), [
      "connect:direct:2",
      "createUser:direct:1",
      "normalize:callback:2",
      "query:direct:1",
      "save:method:2",
      "validate:direct:2",
    ]);
  });

  test("a callee imported through a barrel is its declaration, with the call site", (root) => {
    const edge = callGraph("javascript")
      .callees.get(handler(root))
      .find((e) => nameOf(e.node) === "createUser");
    equal(
      [edge.node.kind(), basename(edge.node.getRoot().filename()), edge.site.text()],
      ["arrow_function", "users.js", "createUser(label, email)"],
    );
  });

  // connect() in db.js, as called from save.
  const connectCall = async (root) => {
    const reached = await crawl(callGraph("javascript").callees, handler(root));
    const save = [...reached.keys()].find((node) => nameOf(node) === "save");
    return save.find({ rule: { pattern: "connect()" } });
  };

  test("a crawl of callers reaches each caller once, at its shortest depth", async (root) => {
    const up = await crawl(callGraph("javascript").callers, await connectCall(root));
    equal(
      [...up]
        .filter(([, edge]) => edge)
        .map(([node]) => nameOf(node) + ":" + depthOf(up, node))
        .sort(),
      ["createUser:2", "handler:2", "query:1", "save:1"],
    );
  });

  test("callers keep an edge the crawl reached a shorter way", async (root) => {
    const { callers } = callGraph("javascript");
    const up = await crawl(callers, await connectCall(root));
    const createUser = [...up.keys()].find((node) => nameOf(node) === "createUser");
    equal(
      callers.get(createUser).map((e) => nameOf(e.node)),
      ["handler"],
    );
  });

  test("edge.from on a callers crawl is the function each caller was reached from", async (root) => {
    const { callees, callers } = callGraph("javascript");
    const fromCall = await crawl(callees, find(root, "query($$$ARGS)"));
    const connect = [...fromCall.keys()].find(
      (node) => fromCall.get(node) && nameOf(node) === "connect",
    );
    const pairs = [...(await crawl(callers, connect))]
      .filter(([, edge]) => edge)
      .map(([node, edge]) => nameOf(node) + "->" + nameOf(edge.from));
    includes(pairs, "query->connect");
    includes(pairs, "handler->query");
  });

  test("stopAt a callback lists it and walks the rest as before", async (root) => {
    const { callees } = callGraph("javascript");
    const all = await crawl(callees, handler(root));
    const callbacks = [...all].filter(([, e]) => e?.via === "callback").map(([node]) => node);
    equal(callbacks.map(nameOf), ["normalize"]);
    equal(walk(await crawl(callees, handler(root), { stopAt: callbacks })), walk(all));
  });

  test("stopAt a method call does not walk past it", async (root) => {
    const { callees } = callGraph("javascript");
    const all = await crawl(callees, handler(root));
    const methods = [...all].filter(([, e]) => e?.via === "method").map(([node]) => node);
    const stopped = await crawl(callees, handler(root), { stopAt: methods });
    includes(walk(stopped), "save:method:2");
    // connect is reached from query only, not through save.
    const connect = [...stopped.keys()].find((node) => nameOf(node) === "connect");
    equal([nameOf(stopped.get(connect).from), depthOf(stopped, connect)], ["query", 2]);
  });

  // The variable `limit`, as a crawl from the argument reaches it, and as declared.
  const limitTwice = async (root) => {
    const reached = await sourcesOf(limit(root));
    const viaFlow = reached.find((e) => e.kind === "variable").node;
    const declared = find(root, "let limit = 10").find({ rule: { kind: "identifier" } });
    return [viaFlow, declared];
  };

  test("the same node from two answers is equal, with the same key, but not identical", async (root) => {
    const [viaFlow, declared] = await limitTwice(root);
    equal(
      [viaFlow === declared, viaFlow.equals(declared), viaFlow.key() === declared.key()],
      [false, true, true],
    );
  });

  test("equals is false for another node and for a non-node", async (root) => {
    const [viaFlow] = await limitTwice(root);
    equal([viaFlow.equals(limit(root)), viaFlow.equals("limit")], [false, false]);
  });

  test("nodeByKey finds the node a key names, of the key's kind", (root) => {
    const json = find(root, "res.json(user)");
    equal(root.nodeByKey(json.key()).equals(json), true, "a call");
    equal(root.nodeByKey(json.parent().key()).kind(), "expression_statement", "same start");
  });

  test("nodeByKey answers null for another file's key", (root) => {
    const key = find(root, "res.json(user)").key().replace("server.js", "other.js");
    equal(root.nodeByKey(key), null);
  });

  test("nodeByKey rejects a string that is not a key", async (root) => {
    await throws(() => root.nodeByKey("not a key"), "is not a node.key()");
  });

  test("contains holds for nodes inside, across answers, and for the node itself", async (root) => {
    const [viaFlow] = await limitTwice(root);
    const call = find(root, "query($$$ARGS)");
    const json = find(root, "res.json(user)");
    equal(
      [
        handler(root).contains(call),
        call.contains(handler(root)),
        handler(root).contains(viaFlow),
        json.contains(json),
      ],
      [true, false, true, true],
    );
  });

  test("crawl starts at a node or a key, not a name", async () => {
    await throws(
      () => crawl(dataflow("javascript").sources, "req"),
      '"req" is not a node or a node.key()',
    );
  });

  test("edge.from leads back to the start, which has no edge", async (root) => {
    const reached = await crawl(dataflow("javascript").sources, limit(root));
    const path = [];
    for (
      let e = [...reached.values()].find((e) => e?.kind === "parameter");
      e;
      e = reached.get(e.from)
    ) {
      path.push(e);
    }
    equal(
      path.slice(0, -1).every((e, i) => e.from.equals(path[i + 1].node)),
      true,
      "each step",
    );
    equal(path[path.length - 1].from.equals(limit(root)), true, "ends at the start");
    equal(reached.get(limit(root)), null, "the start");
  });
});

onFile("cart.js", () => {
  const declared = (root, name) =>
    root
      .root()
      .find({
        rule: { kind: "variable_declarator", has: { field: "name", regex: "^" + name + "$" } },
      })
      .field("name");
  const audit = (root) => find(root, "audit(total)");
  const mutations = (list) =>
    list.map(
      (m) =>
        m.kind +
        ":" +
        m.confidence +
        (m.viaAlias ? ":alias" : "") +
        (m.deferred ? ":deferred" : "") +
        " " +
        m.node.text().split("\n")[0],
    );

  test("mutations include calls, escapes, writes through an alias and deferred writes", (root) => {
    equal(mutations(declared(root, "items").mutations()), [
      'method-call:likely items.push({ sku: "gift" })',
      "escape:maybe sum(items)",
      "method-call:likely:alias snapshot.sort(byPrice)",
      "property-write:definite:deferred items.length = 0",
    ]);
  });

  test("mutations after a node leave out the ones before it, not deferred ones", (root) => {
    equal(mutations(declared(root, "items").mutations({ after: audit(root) })), [
      "method-call:likely:alias snapshot.sort(byPrice)",
      "property-write:definite:deferred items.length = 0",
    ]);
  });

  test("a reassignment is a mutation", (root) => {
    equal(mutations(declared(root, "count").mutations({ after: audit(root) })), [
      "reassign:definite count = count + coupons.length",
    ]);
  });

  test("a value only handed to a call is not mutated after it", (root) => {
    equal(declared(root, "total").mutations({ after: audit(root) }), []);
  });

  test("an escape has a null name, not undefined", (root) => {
    const escape = declared(root, "items")
      .mutations()
      .find((m) => m.kind === "escape");
    equal(escape.name, null);
  });

  test("a call edge has a null index", async (root) => {
    // audit(total): total is sum(items).
    const reached = await sourcesOf(find(root, "audit(total)").field("arguments").child(1));
    const call = reached.find((e) => e.kind === "call");
    equal([call.node.text(), call.index], ["sum(items)", null]);
  });

  test("a lookup rejects a non-node", async () => {
    await throws(() => callGraph("javascript").callees.get(true), "callees: expected an SgNode");
  });

  test("mutations({ after }) asks for an edge's node", async (root) => {
    const items = find(root, "items");
    const edge = dataflow("javascript").sources.get(items)[0];
    await throws(() => items.mutations({ after: edge }), "pass its `.node`");
  });

  test("mutations({ after }) rejects a node of another file", async (root) => {
    const other = parse("javascript", "audit(total)").root();
    await throws(() => find(root, "items").mutations({ after: other }), "not in this node's file");
  });

  test("crawl takes undefined options", async (root) => {
    const items = find(root, "items");
    equal((await crawl(dataflow("javascript").sources, items, undefined)).has(items), true);
  });

  test("crawl rejects a misspelled option instead of crawling everything", async (root) => {
    await throws(
      () => crawl(dataflow("javascript").sources, find(root, "items"), { stopat: [audit(root)] }),
      "unknown option `stopat`; expected `stopAt`",
    );
  });

  test("sources and uses of a statement throw: there is no value there", async (root) => {
    const { sources, uses } = dataflow("javascript");
    const statement = root.root().find({ rule: { kind: "export_statement" } });
    await throws(
      () => sources.get(statement),
      "sources.get(node) on `export_statement`: there is no value here",
    );
    await throws(() => uses.get(statement), "on `export_statement`: there is no value here");
  });

  test("mutations of a literal throw: it is not a variable", async (root) => {
    await throws(
      () => find(root, '"gift"').mutations(),
      "mutations() on `string`: this is not a variable",
    );
  });

  test("mutations of a member ask about its object", async (root) => {
    await throws(() => find(root, "cart.items").mutations(), "for `a.b`, ask about `a`");
  });

  test("a member's property answers for the member", (root) => {
    const property = find(root, "cart.items").field("property");
    equal(dataflow("javascript").sources.get(property)?.length > 0, true);
  });

  test("a node parsed from a string has no file to analyze", async () => {
    const { sources, uses } = dataflow("javascript");
    const loose = parse("javascript", "const a = b;")
      .root()
      .find({ rule: { pattern: "b" } });
    await throws(() => sources.get(loose), "parsed from a string");
    await throws(() => uses.get(loose), "parsed from a string");
    await throws(
      () => symbols("javascript").resolveModule(loose, "./db.js"),
      "resolveModule(file): this root was parsed from a string and has no file",
    );
  });

  test("a lookup rejects a node of another language", async () => {
    const rust = parse("rust", "fn a() { b(); }")
      .root()
      .find({ rule: { kind: "identifier" } });
    await throws(
      () => callGraph("javascript").callers.get(rust),
      "callers.get(node): this lookup is for JavaScript/TypeScript, and the node is Rust",
    );
  });

  test("an unsupported language has no lookups", async () => {
    await throws(
      () => callGraph("rust"),
      'callGraph("rust"): semantic analysis covers "typescript", "tsx", "javascript" and "python"',
    );
  });
});

onFile("view.jsx", () => {
  const attribute = (root, name) =>
    root
      .root()
      .findAll({ rule: { kind: "jsx_attribute" } })
      .find((a) => a.text().startsWith(name + "="));

  test("sources of a JSX prop reach the call and members it was built from", async (root) => {
    const title = await sourcesOf(attribute(root, "title"));
    equal(describe(title.filter((s) => s.boundary || s.kind === "member")), [
      "call:formatName(user.first, user.last)!",
      "member:user.first",
      "member:user.last",
      "parameter:user!",
    ]);
  });

  test("a value that only picks between literals does not flow into the prop", async (root) => {
    const className = await sourcesOf(attribute(root, "className"));
    equal(describe(className.filter((s) => s.kind === "literal" || s.kind === "parameter")), [
      'literal:"card dark"',
      'literal:"card"',
    ]);
  });

  // onClose closes over `user` but is a new function: what it returns
  // leaves through the return, so the crawl stops at returns and stores.
  test("uses of a prop reach the attributes it feeds, not a closure's", async (root) => {
    const { uses } = dataflow("javascript");
    const user = root
      .root()
      .find({ rule: { kind: "shorthand_property_identifier_pattern", regex: "^user$" } });
    const leaves = [...(await crawl(uses, user)).values()]
      .filter((edge) => edge?.via === "return" || edge?.via === "store")
      .map((edge) => edge.node);
    const reached = await crawl(uses, user, { stopAt: leaves });
    equal(
      [...reached.values()].filter((e) => e?.kind === "jsx-attribute").map((e) => e.name),
      ["title"],
    );
  });
});

onFile("patterns.js", () => {
  const member = (root, kind, field, name) =>
    root
      .root()
      .find({ rule: { kind, has: { field, regex: "^" + name + "$" } } })
      .field(field);

  test("sources of an array-destructured name include its default and the call", async (root) => {
    const found = describe(await sourcesOf(argOf(root, "send(second)")));
    includes(found, "variable:second");
    includes(found, "call:pair(req.query.id)!");
    includes(found, 'literal:"none"');
    includes(found, "parameter:req!");
  });

  test("sources of an object-destructured name reach the object", async (root) => {
    const rest = await sourcesOf(argOf(root, "send(rest)"));
    equal(
      rest.some((s) => s.kind === "parameter" && s.name === "obj"),
      true,
      describe(rest).join(),
    );
  });

  test("a for-of destructuring target comes from the iterable and earlier assignments", async (root) => {
    const found = describe(await sourcesOf(argOf(root, "send(first)")));
    includes(found, "parameter:entries!");
    includes(found, "call:pair(req.query.id)!");
  });

  test("each destructuring assignment is a reassignment", (root) => {
    equal(
      member(root, "variable_declarator", "name", "first")
        .mutations()
        .map((m) => m.kind + " " + m.node.text()),
      [
        'reassign [first, second = "none"] = pair(req.query.id)',
        "reassign [first]",
        "escape send(first)",
      ],
    );
  });

  test("class methods and arrow-function fields resolve through this", async (root) => {
    const start = member(root, "method_definition", "name", "start").parent();
    // A method's name, or the field's for an arrow-function field.
    const name = (node) => (node.field("name") ?? node.parent().field("property")).text();
    const reached = await crawl(callGraph("javascript").callees, start);
    equal(
      [...reached]
        .filter(([, edge]) => edge)
        .map(([node, edge]) => name(node) + ":" + edge.via + ":" + depthOf(reached, node)),
      ["onTick:method:1", "#advance:method:2", "report:method:3"],
    );
  });

  test("a field initializer is stored in the field", (root) => {
    const log = root.root().find({ rule: { kind: "array", inside: { kind: "field_definition" } } });
    const stored = (dataflow("javascript").uses.get(log) ?? []).find((e) => e.via === "store");
    equal(stored?.name, "log");
  });

  test("a computed method name has no callee", (root) => {
    const dispatch = member(root, "method_definition", "name", "dispatch").parent();
    equal(callGraph("javascript").callees.get(dispatch), undefined);
  });

  test("reading a field does not see its initializer: the trail ends at this", async (root) => {
    const found = describe(await sourcesOf(argOf(root, "emit(this.count)")));
    includes(found, "this:this!");
    excludes(found, "literal:0");
  });
});
