import { files } from "codemod/files";
import { kind, directory, defer } from "codemod-internal:fs-compat-host";
import { Buffer } from "buffer";

const encoding = (options) => (typeof options === "string" ? options : options?.encoding);
const missing = (path) =>
  Object.assign(new Error(`ENOENT: no such file or directory, '${path}'`), { code: "ENOENT" });
const stats = (type) => ({
  isFile: () => type === "file",
  isDirectory: () => type === "directory",
  isSymbolicLink: () => false,
});
export const constants = Object.freeze({ F_OK: 0, R_OK: 4 });
export const { F_OK, R_OK } = constants;
export const existsSync = files.exists;
export function accessSync(path, mode = F_OK) {
  if (mode !== F_OK && mode !== R_OK) throw new Error("Only read access is supported in workflows");
  if (!files.exists(path)) throw missing(path);
}
export function statSync(path, options) {
  const type = kind(path);
  if (type === "missing") {
    if (options?.throwIfNoEntry === false) return undefined;
    throw missing(path);
  }
  return stats(type);
}
export function readFileSync(path, options) {
  if (!files.exists(path)) throw missing(path);
  const text = files.read(path);
  const format = encoding(options);
  if (!format) return Buffer.from(text);
  if (/^utf-?8$/i.test(format)) return text;
  return Buffer.from(text).toString(format);
}
export function writeFileSync(path, data, options) {
  if (options?.flag && options.flag !== "w" && options.flag !== "wx") {
    throw new Error("Workflow fs writes can only create new files");
  }
  files.create(path, typeof data === "string" ? data : Buffer.from(data).toString("utf8"));
}
export function readdirSync(path, options) {
  return JSON.parse(directory(path)).map(([name, type]) =>
    options?.withFileTypes ? { name, ...stats(type) } : name,
  );
}

function unsupported(name) {
  return () => {
    throw new Error(
      `fs.${name} is not available in workflow callbacks (function steps, forEachFile callbacks and reducers): use files.create() for new files or a shell() step for other changes`,
    );
  };
}

export const mkdirSync = unsupported("mkdirSync");
export const unlinkSync = unsupported("unlinkSync");

const asyncCall =
  (fn) =>
  async (...args) => {
    await defer();
    return fn(...args);
  };
export const promises = Object.freeze({
  access: asyncCall(accessSync),
  stat: asyncCall(statSync),
  readFile: asyncCall(readFileSync),
  writeFile: async (...args) => writeFileSync(...args),
  readdir: asyncCall(readdirSync),
  mkdir: asyncCall(unsupported("mkdir")),
  unlink: asyncCall(unsupported("unlink")),
  constants,
});
const callbackForm =
  (fn) =>
  (...args) => {
    const last = args.at(-1);
    if (typeof last !== "function") return fn(...args);
    args.pop();
    fn(...args).then(
      (result) => last(null, result),
      (error) => last(error),
    );
  };
export const access = callbackForm(promises.access);
export const stat = callbackForm(promises.stat);
export const readFile = callbackForm(promises.readFile);
export const writeFile = callbackForm(promises.writeFile);
export const readdir = callbackForm(promises.readdir);
export const mkdir = callbackForm(promises.mkdir);
export const unlink = callbackForm(promises.unlink);
export default {
  access,
  accessSync,
  constants,
  F_OK,
  R_OK,
  existsSync,
  stat,
  statSync,
  readFile,
  readFileSync,
  writeFile,
  writeFileSync,
  readdir,
  readdirSync,
  mkdir,
  mkdirSync,
  unlink,
  unlinkSync,
  promises,
};
