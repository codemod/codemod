// `codemod/runtime` in Node.js and Bun. Progress and warnings become the
// step's log lines, written as the CLI writes them; failures throw, and stay
// reported even when the code catches the error.

import { context } from "./context.mjs";

function text(message, what) {
  if (typeof message !== "string") throw new TypeError(`${what} expects a message string`);
  return message;
}

function log(prefix, message, meta) {
  const store = context.getStore();
  if (!store) return;
  const line = meta === undefined ? message : `${message} ${JSON.stringify(meta)}`;
  store.run.log(store.step?.position ?? null, `${prefix} ${line}`);
}

export function progress(message, meta) {
  log("[progress]", text(message, "progress()"), meta);
}

export function warn(message, meta) {
  log("[warn]", text(message, "warn()"), meta);
}

export function setCurrentUnit(_unitId, _meta) {}

function fail(message, what) {
  const step = context.getStore()?.step;
  if (step) step.failure = text(message, what);
  throw new Error(text(message, what));
}

export function failFile(message, _meta) {
  fail(message, "failFile()");
}

export function failStep(message, _meta) {
  fail(message, "failStep()");
}

export function isCanceled() {
  return context.getStore()?.run.canceled() ?? false;
}

const runtime = { progress, warn, setCurrentUnit, failFile, failStep, isCanceled };
export default runtime;
