// Second-level helper: proves transitive imports are bundled into an artifact.
export function posixPath(path: string): string {
  return path.replaceAll("\\", "/");
}
