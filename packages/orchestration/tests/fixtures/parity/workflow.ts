// JSSG edge cases (`tests/jssg-parity.e2e.test.ts`): one JSSG
// command per run, picked by the workflow input `{ step, input?, include? }`.
import { dynamic, type Json } from "../../../src/index.ts";
import {
  encoding,
  multi,
  renameInto,
  returns,
  secondaryAt,
  selected,
  skipping,
  throwing,
} from "./transforms.ts";

const steps = { encoding, multi, renameInto, returns, secondaryAt, selected, skipping, throwing };

interface Case {
  step: keyof typeof steps;
  input?: Json;
  include?: string[];
}

export default dynamic(async ({ step, input, include }: Case) => {
  const runnable = steps[step] as unknown as (options: object) => PromiseLike<unknown>;
  return runnable({
    ...(input === undefined ? {} : { input }),
    ...(include === undefined ? {} : { target: { include } }),
  });
});
