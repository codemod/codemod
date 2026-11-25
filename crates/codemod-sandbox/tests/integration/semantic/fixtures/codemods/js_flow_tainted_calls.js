// "Every call that receives a value derived from req.body."
function callsReceiving(source, options) {
  const calls = new Set();
  for (const step of source.flowsTo(options)) {
    if (step.via === "argument") {
      calls.add(step.node.text() + "#" + step.index);
    }
  }
  return [...calls].sort();
}

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

export default function transform(root) {
  const body = root.root().find({ rule: { pattern: "req.body" } });
  if (!body) throw new Error("req.body not found");
  expectEqual(body.flowsTo().truncated, false, "truncated");

  // Calls whose result depends on a tainted argument pass the taint on:
  // `user` comes from createUser(label, email), so log(user) and
  // res.json(user) receive it too. query(...) only sees req.params and req.query.
  expectEqual(
    callsReceiving(body),
    ["createUser(label, email)#0", "createUser(label, email)#1", "log(user)#0", "res.json(user)#0"],
    "derived from req.body",
  );

  // Only the calls that receive the value directly.
  expectEqual(
    callsReceiving(body, { throughCalls: false }),
    ["createUser(label, email)#0", "createUser(label, email)#1"],
    "directly from req.body",
  );

  return null;
}
