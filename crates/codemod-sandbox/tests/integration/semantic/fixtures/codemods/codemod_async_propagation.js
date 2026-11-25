// loadConfig() switches from readFileSync to fs/promises, so it becomes async.
// Every function that calls it, directly or through other functions, must
// await it and become async too, across files. A call that hands the
// function to code we cannot see (`names.map(getSetting)`) gets a TODO.
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

export default function transform(root) {
  const fn = root.root().find({
    rule: { kind: "function_declaration", has: { field: "name", regex: "^loadConfig$" } },
  });
  if (!fn) return null;

  const callers = fn.callers({ transitive: true });
  if (callers.truncated || callers.unparsed.length > 0) {
    throw new Error("call graph incomplete; not propagating async");
  }

  // Edits grouped by file; each function and call edited once.
  const files = new Map();
  const seen = new Set();
  const editsFor = (r) => {
    const key = r.filename();
    if (!files.has(key)) files.set(key, { root: r, edits: [] });
    return files.get(key).edits;
  };
  const once = (r, node) => {
    const key = r.filename() + ":" + node.range().start.index + ":" + node.kind();
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  };

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

  for (const c of callers) {
    if (!c.callSite) continue;
    const site = c.callSite;
    if (!once(site.root, site.node)) continue;
    if (c.via === "callback") {
      editsFor(site.root).push(
        site.node.replace(
          `/* TODO(async): ${c.callSite.node.text().match(/\((\w+)\)/)?.[1] ?? "callee"} is async now; this call gets promises */ ${site.node.text()}`,
        ),
      );
      continue;
    }
    editsFor(site.root).push(awaitCall(site.node));
    if (c.resolved && once(c.root, c.node)) {
      const edit = makeAsync(c.node);
      if (edit) editsFor(c.root).push(edit);
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
