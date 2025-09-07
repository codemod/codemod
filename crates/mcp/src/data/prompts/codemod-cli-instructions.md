The Codemod CLI is a self-hostable workflow engine designed for running large-scale code transformation jobs. It provides:

- **Multiple transformation approaches**: YAML-based ast-grep rules, TypeScript-based JSSG codemods, shell scripts, and hybrid workflows
- **Workflow orchestration**: Define complex multi-step transformations with conditional logic
- **Language support**: JavaScript, TypeScript, Python, Go, Rust, and many more
- **Registry integration**: Publish and share codemods with the community
- **Testing framework**: Built-in testing capabilities for reliable transformations

## Installation and Setup

### Prerequisites

- Basic understanding of code transformations
- Familiarity with YAML for workflow configuration

### Quick Start

Install and run Codemod CLI using npx:

```bash
# Run a codemod from the registry
npx codemod@latest run <package-name>

# Initialize a new codemod project - This command is interactive. You should supply options for a non-interactive experience
npx codemod@latest init

# View all available commands
npx codemod@latest --help
```

## Project Types

When initializing a new codemod project, you can choose from several project types:

### 1. JavaScript AST-Grep (`ast-grep-js`)

TypeScript-based codemods using the JSSG framework:

```bash
npx codemod@latest init ./path/to/dir \
  --name my-jssg-codemod \ # project name
  --project-type ast-grep-js \ # required for non-interactive
  --package-manager pnpm \
  --language typescript \ # required for non-interactive
  --no-interactive
```

Best for:
- Complex TypeScript/JavaScript transformations
- Type-safe AST manipulations
- Reusable utility functions

### 2. Hybrid Workflow (`hybrid`)

Multi-step workflows combining different transformation approaches:

```bash
npx codemod@latest init my-hybrid-codemod \
  --name my-codemod \
  --project-type hybrid \
  --language typescript \
  --no-interactive
```

Best for:
- Complex transformations requiring multiple tools
- Combining ast-grep with shell commands
- Orchestrating multiple transformation steps

### 3. YAML AST-Grep (`ast-grep-yaml`) - Legacy

Pure YAML-based ast-grep rules:

```bash
npx codemod@latest init my-yaml-codemod \
  --project-type ast-grep-yaml \
  --language javascript \
  --no-interactive
```

Best for:
- Simple pattern-based transformations
- Quick rule-based changes
- No custom logic needed

### 4. Shell Command (`shell`) - Legacy

Shell script-based transformations:

```bash
npx codemod@latest init my-shell-codemod \
  --project-type shell
  --no-interactive
```

Best for:
- File operations
- Integration with external tools
- Simple text replacements

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
          # Your analysis logic here

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

For more information about how to write ast-grep YAML rules or JSSG codemods, check the corresponding resources.

## CLI Commands

### Core Commands

#### Initialize a Project

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

#### Workflow Management

```bash
# List available workflows
npx codemod@latest workflow list

# Run a workflow
npx codemod@latest workflow run workflow.yaml

# Validate workflow syntax
npx codemod@latest workflow validate workflow.yaml
```

#### Testing

```bash
# Test JSSG codemods
npx codemod@latest jssg test -l typescript ./codemods/transform.ts

# Test with specific directory
npx codemod@latest jssg test -l typescript ./codemods/transform.ts ./tests

# Update test snapshots
npx codemod@latest jssg test -l typescript ./codemods/transform.ts -u
```

#### Publishing

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
   npm test
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

### 4. Testing Strategy

**Create comprehensive test coverage**:

```
tests/
├── basic/
│   ├── input.ts
│   └── expected.ts
├── edge-cases/
│   ├── empty-file/
│   ├── syntax-errors/
│   └── complex-nesting/
└── integration/
    └── full-project/
```

## Language Support

The workflow engine supports the following languages:

- JavaScript (`javascript`, `js`)
- TypeScript (`typescript`, `ts`)
- JSX (`jsx`)
- TSX (`tsx`) <-- Remember, tsx is different from ts
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
