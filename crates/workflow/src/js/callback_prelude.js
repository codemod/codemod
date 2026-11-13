// ---- appended by the build to every extracted callback or function step ----

let __cb;
let __lang;

function __freeze(value) {
  if (value && typeof value === "object" && !Object.isFrozen(value)) {
    Object.freeze(value);
    for (const key of Object.keys(value)) __freeze(value[key]);
  }
  return value;
}

function __checkJson(value, path, seen = new Set()) {
  if (value === null || typeof value === "string" || typeof value === "boolean") return;
  if (typeof value === "number") {
    if (!Number.isFinite(value)) throw new TypeError(`${path} is ${value}, which is not JSON`);
    return;
  }
  if (typeof value !== "object") {
    const what = value === undefined ? "undefined" : `a ${typeof value}`;
    throw new TypeError(`${path} is ${what}, which is not JSON`);
  }
  if (seen.has(value)) throw new TypeError(`${path} is a circular reference`);
  seen.add(value);
  if (Array.isArray(value)) {
    value.forEach((item, index) => __checkJson(item, `${path}[${index}]`, seen));
  } else {
    const proto = Object.getPrototypeOf(value);
    if (proto !== Object.prototype && proto !== null) {
      const name = (value.constructor && value.constructor.name) || "non-plain object";
      throw new TypeError(`${path} is a ${name}, which is not JSON`);
    }
    for (const key of Object.keys(value)) __checkJson(value[key], `${path}.${key}`, seen);
  }
  seen.delete(value);
}

function __describe(error) {
  if (!error || typeof error !== "object") return String(error);
  let message = `${error.name || "Error"}: ${error.message}`;
  if (
    error instanceof TypeError &&
    /extensible|read-only|read only|frozen|non-writable/i.test(error.message)
  ) {
    message += " (captured values and step results are frozen; copy one before changing it)";
  }
  return message;
}

export function __init(captures, language) {
  __lang = language ?? undefined;
  __cb = __make(__freeze(JSON.parse(captures)));
}

/** Run the callback for one file; never throws. */
async function __one(path, text) {
  const effects = [];
  let open = true;
  const guard = (name) => {
    if (!open) throw new Error(`file.${name}() was called after the callback finished`);
  };
  const file = Object.freeze({
    path,
    text,
    language: __lang,
    write(next) {
      guard("write");
      if (typeof next !== "string") throw new TypeError("file.write(text) expects a string");
      effects.push({ op: "write", text: next });
    },
    rename(to) {
      guard("rename");
      if (typeof to !== "string" || to === "") {
        throw new TypeError("file.rename(path) expects a non-empty path");
      }
      effects.push({ op: "rename", to });
    },
    delete() {
      guard("delete");
      effects.push({ op: "delete" });
    },
  });
  let output;
  try {
    output = await __cb(file);
    if (output !== undefined) __checkJson(output, "return value");
  } catch (error) {
    return { ok: false, error: __describe(error) };
  } finally {
    open = false;
  }
  return {
    ok: true,
    effects,
    output: output === undefined ? null : output,
    hasOutput: output !== undefined,
  };
}

/** Run the callback for a batch of `[path, text]` pairs, concurrently. */
export function __runBatch(batch) {
  return Promise.all(JSON.parse(batch).map(([path, text]) => __one(path, text))).then((outcomes) =>
    JSON.stringify(outcomes),
  );
}

/** Run a function step: call it with no arguments; never throws. */
export async function __runStep() {
  try {
    let result = await __cb();
    if (result === undefined) result = null;
    __checkJson(result, "the step's result");
    return JSON.stringify({ ok: true, result });
  } catch (error) {
    const message =
      error && typeof error === "object" && "message" in error ? error.message : String(error);
    return JSON.stringify({ ok: false, error: message });
  }
}
