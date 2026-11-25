import { getSetting, isDebug } from "./settings.js";

export async function start() {
  if (await isDebug()) {
    console.log("debug on");
  }
  return await getSetting("port");
}

export function allFlags(names) {
  return /* TODO(async): getSetting is async now; this call gets promises */ names.map(getSetting);
}
