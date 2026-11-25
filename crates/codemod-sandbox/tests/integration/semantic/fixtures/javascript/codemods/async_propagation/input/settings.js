import { loadConfig } from "./config.js";

export function getSetting(name) {
  const config = loadConfig("app.json");
  return config[name];
}

export const isDebug = () => getSetting("debug") === true;
