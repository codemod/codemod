// Generated from the Rust types by `cargo test -p codemod protocol_samples`
// (UPDATE_PROTOCOL_SAMPLES=1 rewrites it); do not edit. `tsc -p type-tests`
// checks it against `codemod/protocol`.

import type { Command, Event, Hello } from "codemod/protocol";

export const hello: Hello = {
  "event": "hello",
  "protocol": 1,
  "cli": "2.0.0",
  "run_id": "8c7000b1",
  "mode": "run",
  "dry_run": false,
  "workflow": "/work/codemod/workflow.ts",
  "target": "/work/app",
  "settings": {
    "capabilities": [
      "fs"
    ],
    "managed_git": false,
    "agent": null,
    "decision_model": "systemone/jev-latest",
    "approve_all": false,
    "approved": [
      "approval:1"
    ]
  },
  "commands": [
    "answer",
    "approve",
    "reject",
    "stop"
  ]
};

export const events: Event[] = [
  {
    "event": "run_started",
    "run_id": "8c7000b1",
    "workflow": "/work/codemod/workflow.ts",
    "resumed": false,
    "target": "/work/app",
    "params": {
      "level": 2
    },
    "dry_run": false,
    "settings": {
      "capabilities": [
        "fs"
      ],
      "managed_git": false,
      "agent": "claude-code",
      "host": "quickjs"
    },
    "seq": 0,
    "ts": "2026-01-01T00:00:00+00:00"
  },
  {
    "event": "step_started",
    "position": 0,
    "step": "0",
    "parent": null,
    "label": "Replace",
    "kind": "forEachFile",
    "seq": 1,
    "ts": "2026-01-01T00:00:00+00:00"
  },
  {
    "event": "step_replayed",
    "position": 1,
    "step": "1",
    "parent": null,
    "label": "Find",
    "kind": null,
    "seq": 2,
    "ts": "2026-01-01T00:00:00+00:00"
  },
  {
    "event": "step_progress",
    "position": 0,
    "step": "0",
    "parent": null,
    "processed": 1,
    "total": 2,
    "seq": 3,
    "ts": "2026-01-01T00:00:00+00:00"
  },
  {
    "event": "step_log",
    "position": 0,
    "step": "2/0",
    "parent": "2",
    "line": "src/a.ts",
    "seq": 4,
    "ts": "2026-01-01T00:00:00+00:00"
  },
  {
    "event": "run_log",
    "line": "from the body",
    "step": null,
    "seq": 5,
    "ts": "2026-01-01T00:00:00+00:00"
  },
  {
    "event": "step_agent",
    "position": 3,
    "step": "3",
    "parent": null,
    "agent": "codex",
    "activity": {
      "event": "text",
      "text": "Done.",
      "delta": false
    },
    "seq": 6,
    "ts": "2026-01-01T00:00:00+00:00"
  },
  {
    "event": "step_retrying",
    "position": 0,
    "step": "0",
    "parent": null,
    "label": "Replace",
    "attempt": 1,
    "attempts": 3,
    "error": "boom",
    "seq": 7,
    "ts": "2026-01-01T00:00:00+00:00"
  },
  {
    "event": "step_diff",
    "position": 0,
    "step": "0",
    "parent": null,
    "path": "src/a.ts",
    "diff": "--- a/src/a.ts\n+++ b/src/a.ts\n@@ -1 +1 @@\n-var a = 1;\n+let a = 1;\n",
    "additions": 1,
    "deletions": 1,
    "seq": 8,
    "ts": "2026-01-01T00:00:00+00:00"
  },
  {
    "event": "step_diff",
    "position": 0,
    "step": "0",
    "parent": null,
    "path": "big.json",
    "diff": "",
    "additions": 9000,
    "deletions": 0,
    "diff_omitted": {
      "bytes": 300000,
      "limit": 262144
    },
    "seq": 9,
    "ts": "2026-01-01T00:00:00+00:00"
  },
  {
    "event": "step_finished",
    "position": 0,
    "step": "0",
    "parent": null,
    "label": "Replace",
    "kind": "forEachFile",
    "status": "partial",
    "error": "1 of 2 files failed",
    "duration_ms": 12,
    "seq": 10,
    "ts": "2026-01-01T00:00:00+00:00"
  },
  {
    "event": "question",
    "id": "approval:1",
    "kind": "approval",
    "step": "1",
    "message": "Ship it?",
    "approval": {
      "kind": "manual",
      "message": "Ship it?"
    },
    "seq": 11,
    "ts": "2026-01-01T00:00:00+00:00"
  },
  {
    "event": "question",
    "id": "uncommitted-changes",
    "kind": "approval",
    "step": null,
    "message": "You have uncommitted changes in /work/app. Do you want to proceed anyway?",
    "approval": {
      "kind": "uncommitted-changes",
      "path": "/work/app",
      "notARepository": false
    },
    "seq": 12,
    "ts": "2026-01-01T00:00:00+00:00"
  },
  {
    "event": "question",
    "id": "agent:3",
    "kind": "agent",
    "step": "3",
    "message": "Which coding agent should run this step?",
    "choices": [
      {
        "name": "codex",
        "label": "Codex",
        "available": true
      }
    ],
    "seq": 13,
    "ts": "2026-01-01T00:00:00+00:00"
  },
  {
    "event": "question_closed",
    "id": "approval:1",
    "answer": "approved",
    "by": "answer",
    "seq": 14,
    "ts": "2026-01-01T00:00:00+00:00"
  },
  {
    "event": "question_closed",
    "id": "agent:3",
    "answer": null,
    "by": "eof",
    "seq": 15,
    "ts": "2026-01-01T00:00:00+00:00"
  },
  {
    "event": "control_error",
    "message": "no question 'approval:9' is open",
    "line": "{\"command\":\"approve\",\"question_id\":\"approval:9\"}",
    "seq": 16,
    "ts": "2026-01-01T00:00:00+00:00"
  },
  {
    "event": "run_diff",
    "path": "src/a.ts",
    "diff": "@@ -1 +1 @@\n-var a = 1;\n+let a = 1;\n",
    "additions": 1,
    "deletions": 1,
    "seq": 17,
    "ts": "2026-01-01T00:00:00+00:00"
  },
  {
    "event": "run_finished",
    "run_id": "8c7000b1",
    "status": "rejected",
    "error": "rejected: Ship it?",
    "awaiting": null,
    "question": null,
    "failed_files": 1,
    "partial_steps": [
      {
        "position": 0,
        "label": "Replace",
        "failed_files": 1,
        "files": 2
      }
    ],
    "output": null,
    "steps": [
      {
        "position": 0,
        "label": "Replace",
        "status": "partial",
        "result": {
          "written": [
            "src/a.ts"
          ]
        }
      }
    ],
    "seq": 18,
    "ts": "2026-01-01T00:00:00+00:00"
  }
];

export const commands: Command[] = [
  {
    "command": "approve",
    "question_id": "approval:1"
  },
  {
    "command": "reject",
    "question_id": "approval:1"
  },
  {
    "command": "answer",
    "question_id": "approval:1",
    "value": "later"
  },
  {
    "command": "answer",
    "question_id": "agent:3",
    "value": "codex"
  },
  {
    "command": "answer",
    "question_id": "agent:3",
    "value": null
  },
  {
    "command": "stop",
    "mode": "cancel"
  },
  {
    "command": "stop"
  }
];
