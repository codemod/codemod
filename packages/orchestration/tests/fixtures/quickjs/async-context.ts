// The QuickJS host's async context, through the real bundle's async
// lowering: prints what `async-scenario.ts` observed at every label.
import { dynamic, getHost } from "../../../src/index.ts";
import { scenario } from "./async-scenario.ts";

export default dynamic(() => scenario(getHost().createAsyncContext<string>()));
