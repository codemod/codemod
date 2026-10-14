import { createHash } from "node:crypto";
import { createServer } from "node:net";
import type { ArtifactRef, JssgArtifact } from "../src/index.ts";

/**
 * Whether this process may open a loopback listener. Sandboxes can deny it
 * (`listen EPERM`); tests that need a real socket skip with that reason.
 */
export function canListen(): Promise<boolean> {
  return new Promise((resolve) => {
    const server = createServer();
    server.once("error", () => resolve(false));
    server.listen(0, "127.0.0.1", () => server.close(() => resolve(true)));
  });
}

export const sha256 = (text: string): string => createHash("sha256").update(text).digest("hex");

/** A well-formed artifact reference for tests that never execute the transform. */
export const ref = (name: string): ArtifactRef => ({ name, hash: sha256(name) });

/** An artifact whose source is the given module text, for executor tests. */
export const artifact = (name: string, source: string): JssgArtifact => ({
  name,
  hash: sha256(source),
  source,
  origin: `${name}.ts`,
});
