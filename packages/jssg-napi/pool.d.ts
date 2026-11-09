export interface TransformChunk {
  batchId: number;
  runId: string;
  artifact: { name: string; hash: string; source: string };
  start: number;
  paths: string[];
  contents: string[];
}

/** One file of a chunk: its `FileOutcome`, or the `JSSG failed for '<path>': ...` message. */
export type ChunkResult = { outcome: unknown } | { error: string };

export interface TransformPool {
  readonly size: number;
  run(chunk: TransformChunk): Promise<ChunkResult[]>;
  close(): Promise<void>;
}

export function poolSize(env?: Record<string, string | undefined>): number;
export function createTransformPool(size?: number): TransformPool;
