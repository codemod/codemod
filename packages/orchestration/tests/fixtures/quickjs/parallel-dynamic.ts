// Async-context propagation: commands issued after awaits inside dynamic()
// bodies that are members of a static parallel(), directly or through a
// nested sequence, must be recorded as `concurrent`, exactly as with
// AsyncLocalStorage.
import { dynamic, parallel, sequence, shell } from "../../../src/index.ts";

const step = (name: string) => shell({ name, command: `echo ${name}` });
const [a1, a2, b1, b2, c1] = ["a1", "a2", "b1", "b2", "c1"].map(step);

export default parallel(
  dynamic(async () => {
    await a1!();
    return (await a2!()).stdout;
  }),
  dynamic(async () => {
    const first = await b1!();
    await Promise.resolve();
    const nested = (await sequence(c1!())) as { stdout: string };
    return [first.stdout, nested.stdout, (await b2!()).stdout];
  }),
);
