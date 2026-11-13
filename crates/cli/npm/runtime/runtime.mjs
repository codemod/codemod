// `codemod:runtime` in Node.js and Bun: progress and warnings become the
// step's log lines; failures throw.

import { context } from "./context.mjs";

function format(message, meta) {
  return meta === undefined ? String(message) : `${message} ${JSON.stringify(meta)}`;
}

function log(message, meta) {
  const store = context.getStore();
  if (!store) return;
  store.run.log(store.step?.position ?? null, format(message, meta));
}

export function progress(message, meta) {
  log(message, meta);
}

export function warn(message, meta) {
  log(`warning: ${message}`, meta);
}

export function setCurrentUnit(_unitId, _meta) {}

export function failFile(message, meta) {
  throw new Error(format(message, meta));
}

export function failStep(message, meta) {
  throw new Error(format(message, meta));
}

export function isCanceled() {
  return context.getStore()?.run.canceled() ?? false;
}

const runtime = { progress, warn, setCurrentUnit, failFile, failStep, isCanceled };
export default runtime;
