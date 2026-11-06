// Async-context propagation: commands issued after awaits inside dynamic()
// bodies that are members of a static parallel() must be recorded as
// `concurrent`, exactly as on Node (AsyncLocalStorage).
import { dynamic, parallel, shell } from "../../../src/index.ts";

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
    const nested = await dynamic(async () => (await c1!()).stdout);
    return [first.stdout, nested, (await b2!()).stdout];
  }),
);
