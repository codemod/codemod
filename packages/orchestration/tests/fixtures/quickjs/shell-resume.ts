// History/replay: the body crashes after `count` when the input is `true`.
// Resuming from that run's history with `false` must replay `count` (not
// re-execute it) and execute only `gate`; a third run only replays. Both
// steps append to runs.log, so re-execution is visible.
import { dynamic, shell } from "../../../src/index.ts";

const count = shell({ name: "count", command: "echo count >> runs.log" });
const gate = shell({ name: "gate", command: "echo gate >> runs.log && echo passed" });

export default dynamic(async (crash: boolean) => {
  await count();
  if (crash) throw new Error("crashed before gate");
  return (await gate()).stdout;
});
