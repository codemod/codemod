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
2. Follow threshold-based path selection (`direct`, `adapt`, `build`).
3. Enforce verification via tests and dry-run summaries.

For command-level guidance:
- Start with `references/index.md`.
- Load only the specific reference file needed for the current task.
