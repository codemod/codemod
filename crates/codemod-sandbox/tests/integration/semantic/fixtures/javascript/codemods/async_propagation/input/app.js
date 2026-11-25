import { getSetting, isDebug } from "./settings.js";

export function start() {
  if (isDebug()) {
    console.log("debug on");
  }
  return getSetting("port");
}

export function allFlags(names) {
  return names.map(getSetting);
}
