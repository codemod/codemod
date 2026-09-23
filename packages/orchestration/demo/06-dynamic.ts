/**
 * A dynamic step: plain TypeScript that decides at run time. Calling a step
 * creates a command; awaiting it runs it. The body inspects the target,
 * migrates only when something is pending, awaits a static parallel group for
 * verification, and returns project data.
 *
 * Run it twice against the same target: the second run finds nothing pending
 * and skips the migration. Static nodes that need no input, like the group
 * below, are awaitable here; a node whose first stage requires input is not,
 * because nothing would supply that input.
 */
import { dynamic, parallel } from "@codemod.com/orchestration";
import { inspect, renameCalls, verifyNoLegacy } from "./lib/steps.ts";
import { migratedCalls } from "./04-parallel.ts";

export default dynamic(async () => {
  const inventory = await inspect();
  const migrated = inventory.pending > 0 ? await renameCalls() : [];
  const [{ remaining }, usage] = await parallel(verifyNoLegacy(), migratedCalls());
  return { pending: inventory.pending, migrated, remaining, usage };
});
