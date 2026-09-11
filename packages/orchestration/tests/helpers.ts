import { createHash } from "node:crypto";
import type { ArtifactRef, JssgArtifact } from "../src/index.ts";

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
