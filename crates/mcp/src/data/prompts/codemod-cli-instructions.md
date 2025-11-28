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
workflow: "workflow.yaml"  # Points to your workflow definition
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
  timeout: 300  # seconds
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
      semantic_analysis: workspace  # Enable workspace-wide analysis
```

**Semantic analysis modes:**
- `file` — Single-file analysis (default, fast)
- `workspace` — Cross-file analysis with import resolution
- `{ mode: workspace, root: "./path" }` — Workspace with custom root

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

# Run a codemod directly
npx codemod jssg run -l typescript ./codemods/transform.ts ./target
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
