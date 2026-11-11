import { expect, it } from "vitest";
import { JsonLinesSink } from "../src/index.ts";

it("writes each run event as one JSON line stamped with the time it happened", () => {
  const lines: string[] = [];
  const sink = new JsonLinesSink(
    (line) => lines.push(line),
    () => 1_700_000_000_000,
  );

  sink.emit({ type: "command.output", commandId: "audit:umami", text: "scanned 12 files\n" });

  expect(lines).toEqual([
    '{"at":1700000000000,"type":"command.output","commandId":"audit:umami","text":"scanned 12 files\\n"}\n',
  ]);
});
