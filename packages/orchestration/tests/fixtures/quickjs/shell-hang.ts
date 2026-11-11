// Cancellation: a long shell step the test aborts with SIGTERM to the host.
// The child records its pid so the test can check it was killed.
import { shell } from "../../../src/index.ts";

export default shell({ name: "hang", command: "echo $$ > hang.pid; sleep 30; echo done" });
