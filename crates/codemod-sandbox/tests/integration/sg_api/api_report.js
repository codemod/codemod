// Exercises the JS-visible SgRoot/SgNode API and returns a JSON report. The
// report is compared against `api_report.snapshot.json`, which was recorded
// from the pre-refactor rquickjs implementation, so any behaviour change in
// the AST core or a facade shows up as a snapshot diff.
import { parse, kind } from "codemod:ast-grep";

function errorOf(fn) {
  try {
    const value = fn();
    return { ok: describe(value) };
  } catch (e) {
    return {
      error: e && e.constructor ? e.constructor.name : typeof e,
      message: String(e && e.message),
    };
  }
}

function describe(value) {
  if (value === null || value === undefined) return value === null ? null : "undefined";
  if (Array.isArray(value)) {
    const out = [];
    for (let i = 0; i < value.length; i++) {
      out.push(i in value ? describe(value[i]) : "<hole>");
    }
    return out;
  }
  if (typeof value === "object" && typeof value.kind === "function" && typeof value.text === "function") {
    return node(value);
  }
  if (typeof value === "object" && typeof value.root === "function") {
    return { root: value.filename() };
  }
  if (typeof value === "object") {
    const out = {};
    for (const key of Object.keys(value)) out[key] = describe(value[key]);
    return out;
  }
  return value;
}

function node(n) {
  if (n === null) return null;
  return { kind: n.kind(), text: n.text(), range: n.range() };
}

function kinds(nodes) {
  return nodes.map((n) => n.kind());
}

export default function transform(root, options) {
  const report = {};
  const top = root.root();

  report.root = {
    filename: root.filename().endsWith("input.ts"),
    relativeFilename: root.relativeFilename(),
    sourceMatches: root.source() === top.text(),
    kind: top.kind(),
    isLeaf: top.isLeaf(),
    isNamed: top.isNamed(),
    isNamedLeaf: top.isNamedLeaf(),
    parent: top.parent(),
    childKinds: kinds(top.children()),
    child0: node(top.child(0)),
    child999: top.child(999),
    idIsNumber: typeof top.id() === "number",
    idStable: top.id() === root.root().id(),
    getRootFilenameMatches: top.getRoot().filename() === root.filename(),
    debug: top.child(0).debug(),
  };

  report.matches = (options.matches || []).map(node);
  report.language = options.language;

  const call = top.find("console.log($A)");
  report.find = {
    call: node(call),
    metaA: node(call.getMatch("A")),
    metaMissing: call.getMatch("ZZZ"),
    isCall: call.is("call_expression"),
    isOther: call.is("identifier"),
    field: node(call.field("function")),
    fieldMissing: call.field("nope"),
    fieldChildren: kinds(call.fieldChildren("arguments")),
    ancestors: kinds(call.ancestors()),
    next: node(call.next()),
    prev: node(call.prev()),
    nextAll: kinds(call.nextAll()),
    prevAll: kinds(call.prevAll()),
  };

  const stmt = call.parent();
  report.siblings = {
    next: node(stmt.next()),
    prev: node(stmt.prev()),
    nextAll: kinds(stmt.nextAll()),
    prevAll: kinds(stmt.prevAll()),
  };

  report.findAll = {
    pattern: top.findAll("console.log($A)").map(node),
    kindNumber: top.findAll(kind("typescript", "string")).map((n) => n.text()),
    rule: top.findAll({ rule: { pattern: "foo($$$ARGS)" } }).map((n) => ({
      text: n.text(),
      args: n.getMultipleMatches("ARGS").map((a) => a.text()),
      none: n.getMultipleMatches("NONE").length,
    })),
    constraints: top
      .findAll({
        rule: { pattern: "console.log($A)" },
        constraints: { A: { regex: "^b" } },
      })
      .map((n) => n.text()),
    utils: top
      .findAll({
        rule: { matches: "is-log" },
        utils: { "is-log": { pattern: "console.log($$$)" } },
      })
      .map((n) => n.text()),
    relational: top
      .findAll({ rule: { kind: "identifier", inside: { kind: "arguments", stopBy: "end" } } })
      .map((n) => n.text()),
  };

  const withTransform = top.find({
    rule: { pattern: "console.log($A)" },
    transform: { B: { substring: { source: "$A", startChar: 1 } } },
  });
  report.transformed = {
    value: withTransform.getTransformed("B"),
    type: typeof withTransform.getTransformed("B"),
    isArray: Array.isArray(withTransform.getTransformed("B")),
    missing: withTransform.getTransformed("C"),
  };

  report.relations = {
    matches: call.matches("console.log($$$)"),
    matchesNot: call.matches("foo($$$)"),
    inside: call.inside({ rule: { kind: "expression_statement" } }),
    has: call.has({ rule: { kind: "identifier" } }),
    precedes: stmt.precedes({ rule: { pattern: "foo($$$)" } }),
    follows: stmt.follows({ rule: { kind: "import_statement" } }),
  };

  report.matcherErrors = {
    boolean: errorOf(() => top.findAll(true)),
    undefinedArg: errorOf(() => top.find(undefined)),
    badRule: errorOf(() => top.findAll({ rule: 5 })),
    unknownField: errorOf(() => top.findAll({ rule: { pattern: "x" }, bogus: 1 })),
    emptyRule: errorOf(() => top.findAll({ rule: {} })),
    array: errorOf(() => top.findAll([1, 2])),
    badKind: errorOf(() => top.findAll({ rule: { kind: "not_a_kind" } })),
    matchesBad: errorOf(() => call.matches(null)),
  };

  const edits = top.findAll("console.log($A)").map((n) => n.replace(`logger.info(${n.getMatch("A").text()})`));
  report.edits = {
    edits,
    committed: top.commitEdits(edits),
    overlapping: top.commitEdits([
      { startPos: 0, endPos: 6, insertedText: "X" },
      { startPos: 3, endPos: 8, insertedText: "Y" },
    ]),
    subnode: stmt.commitEdits([call.getMatch("A").replace("'z'")]),
    empty: top.commitEdits([]),
  };

  report.unicode = node(top.find("'héllo'"));

  report.semantic = {
    definition: call.definition(),
    definitionWithOptions: call.definition({ resolveExternal: false }),
    references: call.references(),
  };

  const parsed = parse("typescript", "let x = 1;");
  report.parse = {
    filename: parsed.filename(),
    relativeFilename: parsed.relativeFilename(),
    source: parsed.source(),
    rootKind: parsed.root().kind(),
    badLang: errorOf(() => parse("not-a-language", "x")),
    writeNoPath: errorOf(() => parsed.write("y")),
    renameNoPath: errorOf(() => parsed.rename("other.ts")),
    renameNoPathTwice: errorOf(() => parsed.rename("again.ts")),
    construct: errorOf(() => new (parsed.constructor)()),
  };

  report.rootErrors = {
    writeCurrent: errorOf(() => root.write("y")),
    renameEmpty: errorOf(() => root.rename("")),
    renameOutside: errorOf(() => root.rename("../../outside.ts")),
    rename: errorOf(() => root.rename("renamed.ts")),
    renameTwice: errorOf(() => root.rename("renamed2.ts")),
  };

  return JSON.stringify(report, null, 2) + "\n";
}
