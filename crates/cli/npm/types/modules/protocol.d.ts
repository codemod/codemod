// `codemod/protocol`: what `codemod … --format jsonl` prints, what
// `--control stdio` reads, and what `workflow attach` replays. For programs
// that drive codemod (a TUI, an agent, CI). Types only.
//
// Written by hand next to the Rust types (`Event` in
// crates/workflow/src/events.rs, `Hello` and `Command` in
// crates/cli/src/control.rs). `type-tests/protocol-samples.ts`, which a Rust
// test writes from those types, checks that the two agree.

declare module "codemod/protocol" {
  /** The protocol's version. It goes up only for changes that break clients. */
  export type ProtocolVersion = 1;

  export type JsonValue =
    | null
    | boolean
    | number
    | string
    | JsonValue[]
    | { [key: string]: JsonValue };

  // ------------------------------------------------------------ hello ---

  /**
   * The first line from a run with `--control stdio`, or from `workflow
   * attach`. Not one of the run's events: no `seq`, not in the journal.
   */
  export interface Hello {
    event: "hello";
    protocol: ProtocolVersion;
    /** The CLI's version. */
    cli: string;
    run_id: string;
    mode: "run" | "resume" | "attach";
    dry_run: boolean;
    workflow: string;
    target: string;
    settings: HelloSettings;
    /** The commands this process reads on stdin; none for `attach`. */
    commands: CommandName[];
  }

  export interface HelloSettings {
    capabilities: string[];
    managed_git: boolean;
    agent: string | null;
    /** `provider/model`, for `codemod/assess`. */
    decision_model: string | null;
    /** `--approve`: every approval is given without asking. */
    approve_all: boolean;
    /** Questions `workflow resume --approve <id>` gave. */
    approved: string[];
  }

  // --------------------------------------------------------- commands ---

  export type CommandName = Command["command"];

  /**
   * One line on stdin. `questionId` is accepted for `question_id`. A command
   * that cannot be followed gets a `control_error` event. Closing stdin
   * pauses the run, as `stop` does.
   */
  export type Command =
    | {
        command: "answer";
        question_id: string;
        /**
         * An approval: `"approve"`, `"reject"`, or `"later"` (manual
         * approvals and interrupted steps only); `true`/`false` too.
         * For an interrupted step, approve to rerun or reject to skip.
         * An agent question: an available agent's id, or `null` for the
         * built-in one.
         */
        value: "approve" | "reject" | "later" | boolean | string | null;
      }
    | { command: "approve"; question_id: string }
    | { command: "reject"; question_id: string }
    /** `"pause"` (the default): resumable. `"cancel"`: over. */
    | { command: "stop"; mode?: "pause" | "cancel" };

  // ----------------------------------------------------------- events ---

  /** Every event of a run carries these. */
  export interface Stamp {
    /** Counts up through the whole run, also across resumes. */
    seq: number;
    /** RFC 3339. */
    ts: string;
  }

  /** Which step an event is about. */
  export interface StepAt {
    /** The step's place in its workflow's list of steps. */
    position: number;
    /**
     * Stable across resumes: the position, after the parent's key for a
     * nested codemod's step (`"3"`, `"3/0"`).
     */
    step: string;
    /** The key of the step running the nested codemod; `null` for the run's own steps. */
    parent: string | null;
  }

  /** A step's task: `forEachFile`, `function`, `approval`, `shell`, `agent`, `codemod`, ... */
  export type StepKind = string;

  export type RunStatus =
    | "running"
    | "awaiting-approval"
    /**
     * Stopped on request (Ctrl-C, `stop`, stdin closed): resume continues it.
     * Paused at an open question, it keeps `awaiting` and `question`.
     */
    | "paused"
    | "succeeded"
    | "failed"
    /** Over; resuming takes `--force`. */
    | "canceled"
    /** An approval was rejected: over, earlier changes kept; resuming takes `--force`. */
    | "rejected";

  export type StepStatus =
    | "started"
    | "skipped"
    | "succeeded"
    | "partial"
    | "failed"
    | "canceled"
    | "awaiting-approval"
    | "rejected"
    | "waiting";

  export interface Omitted {
    bytes: number;
    limit: number;
  }

  export type Approval =
    | { kind: "manual"; message: string }
    | { kind: "shell-command"; command: string }
    | { kind: "capabilities"; modules: string[] }
    | { kind: "pull-request"; title: string; branch: string }
    | { kind: "interrupted-step"; label: string; position: number }
    | { kind: "uncommitted-changes"; path: string; notARepository: boolean };

  export interface AgentChoice {
    name: string;
    label: string;
    available: boolean;
  }

  export interface FileChange {
    path: string;
    kind: "create" | "write" | "update" | "delete";
    old?: string;
    new?: string;
    diff?: string;
  }

  /** What a coding agent did (`AgentEvent` in crates/agents/src/event.rs). */
  export type AgentActivity =
    | { event: "started"; session_id?: string; model?: string }
    | { event: "text"; text: string; delta: boolean }
    | { event: "thinking"; text: string }
    | { event: "tool_call"; id?: string; name: string; input: JsonValue; changes?: FileChange[] }
    | { event: "tool_result"; id?: string; output: string; is_error: boolean }
    | { event: "warning"; message: string }
    | { event: "error"; message: string }
    | { event: "output"; stream: "stdout" | "stderr"; line: string };

  export interface PartialStep {
    position: number;
    label: string;
    failed_files: number;
    files: number;
  }

  export interface StepResult {
    position: number;
    label: string;
    status: StepStatus;
    result: JsonValue;
    result_omitted?: Omitted;
  }

  export type RunEvent =
    | {
        event: "run_started";
        run_id: string;
        workflow: string;
        resumed: boolean;
        target: string;
        params: JsonValue;
        dry_run: boolean;
        settings: {
          capabilities: string[];
          managed_git: boolean;
          agent: string | null;
          /** `"quickjs"` (the CLI) or `"node"`. */
          host: string | null;
        };
      }
    | ({ event: "step_started"; label: string; kind: StepKind | null } & StepAt)
    | ({ event: "step_replayed"; label: string; kind: StepKind | null } & StepAt)
    | ({ event: "step_progress"; processed: number; total: number } & StepAt)
    | ({ event: "step_log"; line: string } & StepAt)
    /** A workflow body's `console` output: the run's own (`step: null`) or a nested codemod's. */
    | { event: "run_log"; line: string; step: string | null }
    | ({ event: "step_agent"; agent: string; activity: AgentActivity } & StepAt)
    | ({
        event: "step_retrying";
        label: string;
        attempt: number;
        attempts: number;
        error: string;
      } & StepAt)
    | ({
        event: "step_finished";
        label: string;
        kind: StepKind | null;
        status: StepStatus;
        error: string | null;
        duration_ms: number;
      } & StepAt)
    /** One file a real run's step changed. Dry runs report `run_diff` at the end instead. */
    | ({
        event: "step_diff";
        path: string;
        /** Empty when over the limit; `diff_omitted` says so. */
        diff: string;
        additions: number;
        deletions: number;
        diff_omitted?: Omitted;
      } & StepAt)
    /** The run waits for an answer (with `--control stdio`). */
    | {
        event: "question";
        /**
         * Stable across resumes: `approval:<step>`, `shell:<step>`,
         * `pull-request:<step>`, `capabilities:<step>`, `agent:<step>`, or
         * `interrupted-step:<step>`, or `uncommitted-changes`.
         */
        id: string;
        kind: "approval" | "agent";
        step: string | null;
        message: string;
        approval?: Approval;
        choices?: AgentChoice[];
      }
    | {
        event: "question_closed";
        id: string;
        /** `"approved"`, `"rejected"`, `"later"`; an agent's id or `null`. */
        answer: string | null;
        by: "answer" | "stop" | "eof";
      }
    | { event: "control_error"; message: string; line: string }
    /** A dry run's change to one file, all steps together, when the run ends. */
    | { event: "run_diff"; path: string; diff: string; additions: number; deletions: number }
    | {
        event: "run_finished";
        run_id: string;
        status: RunStatus;
        error: string | null;
        /**
         * What the approval the run waits for asks: with status
         * `"awaiting-approval"`, or `"paused"` when it was paused (stdin
         * closed, `stop`) while the question was open.
         */
        awaiting: string | null;
        /** Its question id: `workflow resume --approve <id>`. */
        question: string | null;
        failed_files: number;
        partial_steps: PartialStep[];
        output: JsonValue;
        output_omitted?: Omitted;
        steps: StepResult[];
      };

  /** A line of `--format jsonl` output after `hello`, or of a run's journal. */
  export type Event = RunEvent & Stamp;

  export type EventName = RunEvent["event"];

  /** Everything a client reads on stdout. */
  export type Line = Hello | Event;
}
