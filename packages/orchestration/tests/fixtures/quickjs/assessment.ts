// A file-oriented assessment in the quickjs runtime: one TypeSafe System One
// request per selected file, through the host's `httpRequest`
// (`TYPESAFE_BASE_URL` points the test at a local stub).
import { assessment } from "../../../src/index.ts";

const judge = assessment({
  name: "judge",
  include: ["*.ts"],
  ask: ({ file }) => ({
    risky: { type: "noul" as const, instructions: `is ${file.path} risky?` },
  }),
});

export default judge();
