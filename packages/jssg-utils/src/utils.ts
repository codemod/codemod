export function stringToExactRegexString(string: string) {
  return `^${string.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`;
}
