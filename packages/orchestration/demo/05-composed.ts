/**
 * The whole migration as one static plan: inspect, analyze in parallel,
 * migrate, verify in parallel. Reading this file is reading the plan; nothing
 * is scheduled dynamically, and the engine can see every step before the
 * first one runs.
 *
 * Data flow is typed end to end. `inspect` yields an `Inventory`; the
 * analyses declare no input and therefore ignore the preceding value, as
 * does the migration. The migration's `Migration[]` is the input of the final
 * group, where `verify-migrations` consumes it and `verifyNoLegacy` ignores
 * it. Moving the verification group before
 * the migration is a type error: it would receive the analyses' tuple instead
 * of `Migration[]`.
 */
import { parallel, sequence } from "@codemod.com/orchestration";
import { inspect, renameCalls, verifyMigrations, verifyNoLegacy } from "./lib/steps.ts";
import { legacyCalls, migratedCalls, unwrappedNewCalls } from "./04-parallel.ts";

export default sequence(
  inspect(),
  parallel(legacyCalls(), migratedCalls(), unwrappedNewCalls()),
  renameCalls(),
  parallel(verifyNoLegacy(), verifyMigrations()),
);
