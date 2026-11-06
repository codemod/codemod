/**
 * Node reference for the quickjs e2e tests: load a workflow through the build
 * step (`loadWorkflow`, Node's module hooks, which vitest's loader bypasses,
 * hence a subprocess) and run it in Node (AsyncLocalStorage) with an executor
 * that answers `shell` like `echo` would. Prints `{ output, history }`.
 *
 * node --experimental-transform-types --import ../../../bin/register-ts.mjs node-reference.ts <workflow.ts>
 */
import {
  MemoryHistoryStore,
  PROTOCOL_VERSION,
  loadWorkflow,
  run,
  type OperationCompletion,
  type OperationRequest,
} from "../../../src/index.ts";

const workflow = process.argv[2]!;
const { exports } = await loadWorkflow(workflow);
const store = new MemoryHistoryStore();
const runAny = run as (executable: unknown, options: unknown) => Promise<{ output: unknown }>;
const result = await runAny(exports.default, {
  history: store,
  executor: {
    execute: async (request: OperationRequest): Promise<OperationCompletion> => ({
      protocolVersion: PROTOCOL_VERSION,
      commandId: request.commandId,
      status: "succeeded",
      output: {
        stdout: `${(request.operation as { command: string }).command.replace(/^echo /u, "")}\n`,
      },
    }),
  },
});
process.stdout.write(JSON.stringify({ output: result.output, history: store.toJSON() }));
