// loadConfig() switches from readFileSync to fs/promises, so it becomes async.
// Every function that calls it, directly or through other functions, must
// await it and become async too, across files: a crawl over the callers
// lookup. A call that hands the function to code we cannot see
// (`names.map(getSetting)`, a callback edge) gets a TODO, and the crawl
// does not walk past it.
import { callGraph, crawl } from "codemod:ast-grep";

function awaitCall(call) {
  const parent = call.parent();
  const needsParens =
    parent && (parent.kind() === "member_expression" || parent.kind() === "call_expression");
  return call.replace(needsParens ? `(await ${call.text()})` : `await ${call.text()}`);
}

function makeAsync(fn) {
  if (fn.kind() === "function_declaration" || fn.kind() === "function_expression") {
    return fn.child(0).replace("async function");
  }
  if (fn.kind() === "arrow_function") {
    const first = fn.child(0);
    return first.replace("async " + first.text());
  }
  return null;
}

// A function's name: its own, or the name it is bound to.
function nameOf(fn) {
  const own = fn.field("name");
  if (own) return own.text();
  const parent = fn.parent();
  const bound = parent && (parent.field("name") || parent.field("key"));
  return bound ? bound.text() : "<anonymous>";
}

export default async function transform(root) {
  const fn = root.root().find({
    rule: { kind: "function_declaration", has: { field: "name", regex: "^loadConfig$" } },
  });
  if (!fn) return null;

  // Every function that calls loadConfig, directly or not, except past a
  // callback edge: what a callback edge reached is not walked further.
  const { callers } = callGraph("javascript");
  const handedOff = [...(await crawl(callers, fn))]
    .filter(([, edge]) => edge?.via === "callback")
    .map(([node]) => node);
  const reached = await crawl(callers, fn, { stopAt: handedOff });
  const asyncFns = [...reached.keys()].filter((node) => !handedOff.some((h) => h.equals(node)));

  // Edits grouped by file; each function and call edited once.
  const files = new Map();
  const seen = new Set();
  const editsFor = (r) => {
    const key = r.filename();
    if (!files.has(key)) files.set(key, { root: r, edits: [] });
    return files.get(key).edits;
  };
  const once = (node) => !seen.has(node.key()) && !!seen.add(node.key());

  const own = editsFor(root);
  own.push(
    ...root
      .root()
      .findAll({ rule: { kind: "import_statement", regex: "readFileSync" } })
      .map((n) =>
        n.replace(
          n.text().replace("readFileSync", "readFile").replace('"node:fs"', '"node:fs/promises"'),
        ),
      ),
  );
  own.push(
    ...fn
      .findAll({ rule: { pattern: "readFileSync($$$ARGS)" } })
      .map((n) => n.replace("await readFile" + n.text().slice("readFileSync".length))),
  );
  own.push(makeAsync(fn));

  // Every call of a function that is now async: the crawl reaches each
  // caller once, the callers lookup has every call site.
  for (const callee of asyncFns) {
    for (const edge of callers.get(callee) ?? []) {
      const site = edge.site;
      if (!once(site)) continue;
      if (edge.via === "callback") {
        const todo = `/* TODO(async): ${nameOf(callee)} is async now; this call gets promises */ `;
        editsFor(site.getRoot()).push(site.replace(todo + site.text()));
        continue;
      }
      editsFor(site.getRoot()).push(awaitCall(site));
      const edit = once(edge.node) && makeAsync(edge.node);
      if (edit) editsFor(edge.node.getRoot()).push(edit);
    }
  }

  let result = null;
  for (const [filename, { root: r, edits }] of files) {
    if (edits.length === 0) continue;
    const content = r.root().commitEdits(edits);
    if (filename === root.filename()) result = content;
    else r.write(content);
  }
  return result;
}
