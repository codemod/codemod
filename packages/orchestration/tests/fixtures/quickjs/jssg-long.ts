// Cancellation mid-batch: a shell step marks the start, then one JSSG
// command whose transform spins ~10 ms per file over every `*.ts` file. The
// test sends SIGINT while the batch runs; the host must cancel it (nothing
// written) instead of finishing it first. A batch awaits only resolved
// promises, so this needs the host's per-chunk yield to its event loop.
import { jssg, sequence, shell } from "../../../src/index.ts";

const started = shell({ name: "started", command: "echo started > started.marker" });

const slow = jssg({
  name: "slow",
  language: "typescript",
  include: ["many/**/*.ts"],
  transform(root) {
    const until = Date.now() + 10;
    while (Date.now() < until) {
      // spin: a transform the JS thread cannot leave
    }
    return root.root().text().replace("oldApi", "newApi");
  },
});

export default sequence(started(), slow());
