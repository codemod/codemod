import type { ArtifactRef } from "./protocol.ts";

/**
 * One bundled transform (`crates/orchestration/src/build` splits it out of the
 * workflow). `source` is what a runtime executes; `hash` identifies it.
 */
export interface JssgArtifact extends ArtifactRef {
  source: string;
  /** Module the transform was extracted from, for messages only. */
  origin: string;
}

/** Artifact lookup by hash; a `Map<string, JssgArtifact>` qualifies. */
export interface ArtifactStore {
  get(hash: string): JssgArtifact | undefined;
}
