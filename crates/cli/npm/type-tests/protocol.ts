// Type tests for `codemod/protocol`: how a program that drives codemod reads
// its output and writes commands. Checked with `pnpm --filter codemod
// typecheck`; nothing here runs. protocol-samples.ts holds what the CLI
// really prints.

import type { Command, Event, Hello, Line } from "codemod/protocol";

declare const line: Line;

if (line.event === "hello") {
  const hello: Hello = line;
  const version: 1 = hello.protocol;
  const mode: "run" | "resume" | "attach" = hello.mode;
  void version;
  void mode;
} else {
  const seq: number = line.seq;
  void seq;
  switch (line.event) {
    case "question": {
      const id: string = line.id;
      const step: string | null = line.step;
      if (line.kind === "approval") {
        const answer: Command = { command: "approve", question_id: id };
        const later: Command = { command: "answer", question_id: id, value: "later" };
        void answer;
        void later;
      }
      void step;
      break;
    }
    case "step_diff": {
      const parent: string | null = line.parent;
      const diff: string = line.diff;
      void parent;
      void diff;
      break;
    }
    case "run_finished": {
      // @ts-expect-error not a status a run has
      const wrong: typeof line.status = "stopped";
      const question: string | null = line.question;
      void wrong;
      void question;
      break;
    }
  }
}

const stop: Command = { command: "stop", mode: "pause" };
// @ts-expect-error a stop is to pause or to cancel
const bad: Command = { command: "stop", mode: "later" };
// @ts-expect-error every event has a number
const unnumbered: Event = { event: "run_log", line: "x", step: null };
void stop;
void bad;
void unnumbered;
