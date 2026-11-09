import { getCleanUrlString } from "lib";

// cleanUrl stays in this comment
const label = "cleanUrl";

export function normalize(url: string) {
  return getCleanUrlString(url);
}

function debug(cleanUrl: string) {
  return cleanUrl.trim();
}

const bag = { getCleanUrlString };
const record = { cleanUrl: 1 };
record.cleanUrl;
