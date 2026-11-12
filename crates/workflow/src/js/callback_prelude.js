// ---- appended by the build to every extracted forEachFile callback ----

let __cb;
let __lang;

const __EXTENSIONS = {
  ts: "typescript",
  mts: "typescript",
  cts: "typescript",
  tsx: "tsx",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  jsx: "javascript",
  css: "css",
  json: "json",
  html: "html",
  py: "python",
  rs: "rust",
  go: "go",
  java: "java",
  yaml: "yaml",
  yml: "yaml",
};

function __flowParse(file, language) {
  if (!file || typeof file.text !== "string" || typeof file.path !== "string") {
    throw new TypeError("parse(file) expects the file object passed to the callback");
  }
  const extension = (file.path.split(".").pop() || "").toLowerCase();
  const lang = language ?? __lang ?? __EXTENSIONS[extension];
  if (!lang) {
    throw new Error(
      `parse(file): no language for '${file.path}'; pass one, or set 'language' on the selector`,
    );
  }
  return __sgParse(lang, file.text);
}

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

export function __run(path, text) {
  const effects = [];
  let open = true;
  const guard = (name) => {
    if (!open) throw new Error(`file.${name}() was called after the callback returned`);
  };
  const file = Object.freeze({
    path,
    text,
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
    output = __cb(file);
  } catch (error) {
    throw new Error(__describe(error));
  } finally {
    open = false;
  }
  if (output && typeof output.then === "function") {
    throw new Error("forEachFile callbacks must be synchronous");
  }
  if (output !== undefined) __checkJson(output, "return value");
  return JSON.stringify({
    effects,
    output: output === undefined ? null : output,
    hasOutput: output !== undefined,
  });
}
