// Helpers a transform may import: bundled into its artifact by the build step.
export { posixPath } from "./format.ts";

export const OLD_API = "oldApi";

export function migrateText(text: string): string {
  return text.replaceAll(OLD_API, "newApi");
}
