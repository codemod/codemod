import { cleanUrl } from "lib";

// cleanUrl stays in this comment
const label = "cleanUrl";

export function normalize(url: string) {
  return cleanUrl(url);
}

function debug(cleanUrl: string) {
  return cleanUrl.trim();
}

const bag = { cleanUrl };
const record = { cleanUrl: 1 };
record.cleanUrl;
