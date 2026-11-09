// A file-oriented assessment in the quickjs runtime: one decision API request
// per selected file, through the host's `httpRequest` (`DECISION_*` or
// `TYPESAFE_*` point the test at a local stub).
import { assessment } from "../../../src/index.ts";

const judge = assessment({
  name: "judge",
  include: ["*.ts"],
  ask: ({ file }) => ({
    risky: { type: "yesno" as const, instructions: `is ${file.path} risky?` },
  }),
});

export default judge();
