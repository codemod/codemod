import { shell } from "../../../src/index.ts";

// A bare root step is an implicit single invocation.
export default shell({ name: "inspect", command: "true" });
