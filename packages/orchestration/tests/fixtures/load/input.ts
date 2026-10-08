import { dynamic, type Json } from "../../../src/index.ts";

// Echoes its root input, so a test can tell an explicit `null` from no input.
export default dynamic((input: Json) => ({ received: input }));
