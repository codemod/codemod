export function importArtifact(
  artifact: { name: string; hash: string; source: string },
  runId: string,
): Promise<unknown>;
