# Codemod CLI Documentation

The Codemod CLI is a self-hostable workflow engine designed for running large-scale code transformation jobs. It provides:

- **Multiple transformation approaches**: TypeScript-based JSSG codemods, shell scripts, and hybrid workflows
- **Workflow orchestration**: Define complex multi-step transformations with conditional logic
- **Language support**: JavaScript, TypeScript, Python, Go, Rust, and many more
- **Registry integration**: Publish and share codemods with the community
- **Testing framework**: Built-in testing capabilities for reliable transformations
- **Semantic analysis**: Cross-file symbol definitions and references (JavaScript/TypeScript and Python)

**For writing codemods**, see the `jssg-instructions` document which covers ast-grep fundamentals, pattern matching, and codemod development.

## Installation and Setup

### Prerequisites

- Node.js with npx (or pnpm/yarn/bun)
- Basic understanding of code transformations
- Familiarity with YAML for workflow configuration

### Quick Start

Install and run Codemod CLI using npx:

```bash
# Run a codemod from the registry
npx codemod@latest run <package-name>

# Initialize a new codemod project
npx codemod@latest init

# View all available commands
npx codemod@latest --help
```

## Project Types

When initializing a new codemod project, you can choose from several project types:

### 1. JavaScript AST-Grep (`ast-grep-js`) - Recommended

TypeScript-based codemods using the JSSG framework:

```bash
npx codemod@latest init ./path/to/dir \
  --name my-jssg-codemod \
  --project-type ast-grep-js \
  --package-manager pnpm \
  --language typescript \
  --no-interactive
```

Best for:

- Complex TypeScript/JavaScript transformations
- Type-safe AST manipulations
- Reusable utility functions

### 2. YAML AST-Grep (`ast-grep-yaml`) - Legacy

Pure YAML-based ast-grep rules:

```bash
npx codemod@latest init my-yaml-codemod \
  --project-type ast-grep-yaml \
  --language javascript \
  --no-interactive
```

### 3. Shell Command (`shell`) - Legacy

Shell script-based transformations:

```bash
npx codemod@latest init my-shell-codemod \
  --project-type shell \
  --no-interactive
```

## Codemod Configuration

The `codemod.yaml` file defines your codemod's metadata and configuration:

```yaml
schema_version: "1.0"

name: "my-awesome-codemod"
version: "0.1.0"
description: "Transform legacy patterns to modern syntax"
author: "Your Name <you@example.com>"
license: "MIT"
workflow: "workflow.yaml" # Points to your workflow definition
category: "migration"
repository: "https://github.com/username/codemod-repo"

targets:
  languages: ["tsx", "ts", "jsx", "js"]

keywords: ["transformation", "migration", "refactoring"]

registry:
  access: "public"
  visibility: "public"
```

### Configuration Fields

- **name**: Unique identifier for your codemod
- **version**: Semantic version (follows semver)
- **workflow**: Path to the workflow definition file
- **targets.languages**: Supported programming languages
- **category**: Classification (migration, refactoring, cleanup, etc.)
- **registry**: Publishing settings

## Workflow Engine

The workflow engine orchestrates multi-step code transformations through a YAML-based configuration.

### Workflow Structure

A `workflow.yaml` file defines the execution flow:

```yaml
version: "1"

# Global configuration
config:
  timeout: 300 # seconds
  continue_on_error: false

# Workflow nodes (execution units)
nodes:
  - id: analyze
    name: Analyze codebase
    type: automatic
    steps:
      - id: find-patterns
        name: Find deprecated patterns
        command: |
          echo "Analyzing codebase..."

  - id: transform
    name: Apply transformations
    type: automatic
    depends_on: [analyze]
    steps:
      - id: run-codemod
        name: Execute main transformation
        js-ast-grep:
          js_file: "scripts/transform.ts"
          base_path: "."
          language: "typescript"
          include:
            - "**/*.ts"
            - "**/*.tsx"
          exclude:
            - "**/node_modules/**"
            - "**/*.test.ts"

  - id: cleanup
    name: Post-processing
    type: automatic
    depends_on: [transform]
    steps:
      - id: format-code
        name: Format transformed files
        command: |
          npx prettier --write "**/*.{ts,tsx,js,jsx}"
```

### Node Types

1. **automatic**: Executes without user intervention (default)
2. **manual**: Requires user confirmation

### Step Types

#### 1. Command Steps

Execute shell commands:

```yaml
steps:
  - id: install-deps
    name: Install dependencies
    command: |
      npm install
      npm run build
```

#### 2. AST-Grep Steps (YAML Rules)

Apply ast-grep rules defined in YAML:

```yaml
steps:
  - id: apply-rules
    name: Apply ast-grep rules
    ast-grep:
      config_file: "rules/transform.yaml"
      base_path: "./src"
      include:
        - "**/*.js"
      exclude:
        - "**/*.test.js"
```

#### 3. JavaScript AST-Grep Steps (JSSG)

Execute TypeScript-based codemods:

```yaml
steps:
  - id: jssg-transform
    name: Run JSSG codemod
    js-ast-grep:
      js_file: "codemods/main.ts"
      base_path: "."
      language: "typescript"
      include:
        - "**/*.ts"
        - "**/*.tsx"
```

#### 4. Semantic Analysis Configuration

Enable cross-file symbol analysis for JSSG codemods (JavaScript/TypeScript and Python only):

```yaml
steps:
  - id: transform-with-semantics
    name: Transform with cross-file analysis
    js-ast-grep:
      js_file: "scripts/codemod.ts"
      semantic_analysis: workspace # Enable workspace-wide analysis
```

**Semantic analysis modes:**

- `file` — Single-file analysis (default, fast)
- `workspace` — Cross-file analysis with import resolution
- `{ mode: workspace, root: "./path" }` — Workspace with custom root

#### 5. AI Steps

Execute AI-powered transformations or reviews:

```yaml
steps:
  - name: "AI Code Review"
    ai:
      model: "gpt-5.2-codex"
      prompt: |
        Review the transformed code and fix any issues...
      system_prompt: |
        You are a code review expert...
```

**AI Step Configuration:**

- `prompt` — The instructions for the AI agent
- `system_prompt` — Optional system context
- `model` — LLM model to use (default: gpt-5.2)
- `max_steps` — Maximum agent steps (default: 100)

**Environment Variables:**

- `LLM_API_KEY` — API key for the LLM provider
- `LLM_MODEL` — Override the model
- `LLM_PROVIDER` — Provider (openai, anthropic, google_ai)
- `LLM_BASE_URL` — Optional custom endpoint URL (inferred from provider if not set)

**Agent Handoff (No API Key):**

When no `LLM_API_KEY` is set, the CLI prints the prompt as `[AI INSTRUCTIONS]` instead of failing:

```
[AI INSTRUCTIONS]

<prompt content here>

[/AI INSTRUCTIONS]
```

This allows coding agents like Claude Code to detect and execute the instructions directly, enabling seamless human-AI collaboration without requiring API keys.

## CLI Commands

### Initialize a Project

```bash
npx codemod@latest init [PATH] [OPTIONS]

Options:
  --name                Project name
  --project-type       Project type (ast-grep-js, hybrid, shell, ast-grep-yaml)
  --package-manager    Package manager (npm, yarn, pnpm)
  --language          Target language
  --description       Project description
  --author           Author information
  --license          License type
  --private          Make package private
  --force            Overwrite existing files
  --no-interactive   Skip interactive prompts
```

### Workflow Management

```bash
# List available workflows
npx codemod@latest workflow list

# Run a workflow on a target directory
npx codemod workflow run -w /path/to/workflow.yaml -t /path/to/target

# Validate workflow syntax
npx codemod@latest workflow validate workflow.yaml
```

### JSSG Commands

```bash
# Test JSSG codemods
npx codemod jssg test -l typescript ./codemods/transform.ts

# Test with specific directory
npx codemod jssg test -l typescript ./codemods/transform.ts ./tests

# Update test snapshots
npx codemod jssg test -l typescript ./codemods/transform.ts -u

# Test with semantic analysis enabled
npx codemod jssg test -l typescript ./codemods/transform.ts --semantic-workspace /path/to/project

# Test with AST comparison (ignores formatting, preserves ordering)
npx codemod jssg test -l typescript ./codemods/transform.ts --strictness ast

# Test with loose comparison (ignores formatting, indentation, unordered children like object properties, and comment positions)
# Note: Python preserves indentation checking since it's semantically significant
npx codemod jssg test -l typescript ./codemods/transform.ts --strictness loose

# Run a codemod directly
npx codemod jssg run -l typescript ./codemods/transform.ts ./target
```

### Running Codemods

```bash
# Run a codemod from the registry
npx codemod run <package-name> [OPTIONS]

Options:
  --target, -t         Target directory (default: current directory)
  --dry-run            Preview changes without modifying files (shows colored diffs)
  --allow-dirty        Allow running on repos with uncommitted changes
  --no-interactive     CI/headless mode - no prompts, auto-accept packages
  --no-color           Disable colored diff output in dry-run mode
  --allow-fs           Allow filesystem access for the codemod
  --allow-fetch        Allow network requests for the codemod
  --allow-child-process Allow spawning child processes
  --param KEY=VALUE    Pass parameters to the codemod (e.g., --param autoAiReview=true)
  --registry           Custom registry URL
  --force              Force re-download even if cached
```

**AI-Powered Codemods:**

Some codemods include optional AI steps (e.g., `class-to-function-component` with `--param autoAiReview=true`). When running AI steps:

- Set `LLM_API_KEY` environment variable to execute AI steps automatically
- Without an API key, the CLI prints `[AI INSTRUCTIONS]` containing the prompt, allowing coding agents like Claude Code to perform those instructions directly

```bash
# With API key - AI step executes automatically
LLM_API_KEY=sk-xxx npx codemod run class-to-function-component --param autoAiReview=true

# Without API key - prints [AI INSTRUCTIONS] for agents to pick up
npx codemod run class-to-function-component --param autoAiReview=true
```

**Examples:**

```bash
# Dry run to preview changes
npx codemod run @org/my-codemod --dry-run --target ./src

# CI/headless mode (no prompts, auto-accept npm packages)
npx codemod run @org/my-codemod --no-interactive

# Full CI pipeline usage
npx codemod run @org/my-codemod \
  --dry-run \
  --no-interactive \
  --target ./src
```

### CI/Headless Mode

For running codemods in CI pipelines or headless environments, use these flags:

| Flag               | Description                                                                                      |
| ------------------ | ------------------------------------------------------------------------------------------------ |
| `--no-interactive` | Disables all prompts. Auto-accepts npm package installations.                                    |
| `--dry-run`        | Skips all `run:` script steps and file modifications. Safe for validation.                       |
| `--allow-dirty`    | Bypasses the git dirty check. Without this flag, the CLI exits if there are uncommitted changes. |

**Git Dirty Check Behavior:**

- Without `--allow-dirty`: CLI **exits with error** if the target has uncommitted changes
- With `--allow-dirty`: CLI proceeds regardless of git state

**Dry Run Behavior:**

- Shows colored unified diffs for each file that would be modified
- Displays addition/deletion counts per file and total summary
- Skips all `run:` script steps in workflows (shell commands are not executed)
- No files are modified on disk
- Use `--no-color` to disable colored output (useful for CI logs)

**Example dry-run output:**

```
============================================================
File: /path/to/src/App.test.js
============================================================
--- [before] /path/to/src/App.test.js
+++ [after]  /path/to/src/App.test.js
@@ -1,3 +1,4 @@
+import { describe, expect, it } from "vitest";
 import App from "./App"
...
+1 additions, -0 deletions

=== DRY RUN SUMMARY ===
Files that would be modified: 8
Total: +10 additions, -2 deletions
No changes were made to the filesystem.
```

### Publishing

```bash
# Login to registry
npx codemod@latest login

# Publish codemod
npx codemod@latest publish

# Search registry
npx codemod@latest search "react migration"

# Run from registry
npx codemod@latest run @scope/codemod-name
```

## Publishing and Distribution

### Preparing for Publication

1. **Update codemod.yaml**:

   ```yaml
   name: "@yourscope/codemod-name"
   version: "1.0.0"
   registry:
     access: "public"
     visibility: "public"
   ```

2. **Add comprehensive documentation**:
   - README.md with usage instructions
   - Examples of transformations
   - Migration guide

3. **Test thoroughly**:
   ```bash
   pnpm test
   npx codemod@latest workflow validate workflow.yaml
   ```

### Publishing Process

```bash
# Login first
npx codemod@latest login

# Publish to registry
npx codemod@latest publish

# Verify publication
npx codemod@latest search "@yourscope/codemod-name"
```

### Using Published Codemods

```bash
# Run directly
npx codemod@latest run @yourscope/codemod-name

# With options
npx codemod@latest run @yourscope/codemod-name \
  --target ./src \
  --dry-run
```

## Language Support

The workflow engine supports the following languages:

- JavaScript (`javascript`, `js`)
- TypeScript (`typescript`, `ts`)
- JSX (`jsx`)
- TSX (`tsx`) — Use TSX for any JSX code
- Python (`python`)
- Go (`go`)
- Rust (`rust`)
- Java (`java`)
- C (`c`)
- C++ (`cpp`)
- C# (`csharp`)
- Ruby (`ruby`)
- PHP (`php`)
- Swift (`swift`)
- Kotlin (`kotlin`)
- Scala (`scala`)
- And more...

**Semantic Analysis Support:** Cross-file symbol analysis (`definition()`, `references()`) is only available for:

- JavaScript/TypeScript (using [oxc](https://oxc.rs/))
- Python (using [ruff](https://docs.astral.sh/ruff/))

Other languages return no-op results for semantic methods.

For more information about ast-grep: https://ast-grep.github.io/llms.txt
