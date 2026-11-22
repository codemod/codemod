// A shard function that type-tests/workflow.ts imports.

import type { ShardInput, ShardResult } from "codemod/workflow";

export default function shardFiles({ files }: ShardInput): ShardResult[] {
  return [{ name: "all", _meta_shard: 0, _meta_files: files }];
}
