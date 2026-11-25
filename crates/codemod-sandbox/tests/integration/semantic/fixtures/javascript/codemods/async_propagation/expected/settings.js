import { loadConfig } from "./config.js";

export async function getSetting(name) {
  const config = await loadConfig("app.json");
  return config[name];
}

export const isDebug = async () => await getSetting("debug") === true;
