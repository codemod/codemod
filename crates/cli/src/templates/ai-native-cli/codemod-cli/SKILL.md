---
name: codemod-cli
description: Orchestrate codemod migrations through codemod CLI routing, suitability thresholds, and verification gates.
allowed-tools:
  - Bash(codemod *)
argument-hint: "<migration-intent>"
---

# Codemod CLI

codemod-compatibility: mcs-v1
codemod-skill-version: 1.0.0

Use this skill as the MCS entrypoint for migration orchestration:
1. Route intent through `codemod agent run "<intent>"`.
2. Read the returned decision and artifact contract (`candidate-evaluation.json`).
3. If decision is `insufficient_metadata` or `no_candidates`, fallback to `codemod search` then `codemod run`.
4. Enforce verification via tests and dry-run summaries before apply.

For command-level guidance:
- Start with `references/index.md`.
- Load only the specific reference file needed for the current task.
