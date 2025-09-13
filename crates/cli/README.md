# Codemod CLI

The Codemod CLI is a command-line interface for running and managing Butterflow workflows - a lightweight, self-hostable workflow engine designed for running large-scale code transformation jobs.

> **NOTE**: This CLI is currently in alpha and may change over time. So be careful when using it in production environments.
> For more information read the [blog post announcing the CLI](https://codemod.com/blog/new-codemod-cli).
> And any feedback is welcome!

## Installation

### Building from Source

```bash
# Clone the repository
git clone https://github.com/codemod-com/codemod.git
cd codemod

# Build the project
cargo build --release

# The executable will be available at target/release/codemod
```

### From npm registry

```bash
npm install -g codemod
```

## Quick Start

1. Create a workflow file `workflow.yaml`:

```yaml
version: "1"
nodes:
  - id: hello-world
    name: Hello World
    type: automatic
    runtime:
      type: direct
    steps:
      - id: hello
        name: Say Hello
        run: echo "Hello, World!"
```

2. Run the workflow:

```bash
codemod run -w workflow.yaml
```

## Commands

### `codemod init`

Initialize a new Butterflow workflow project with interactive setup:

```bash
# Initialize a new project in current directory
codemod init

# Initialize a new project in a specific directory
codemod init my-project/

# Initialize with a specific name
codemod init --name my-workflow
```

**Options:**
- `--name <NAME>`: Set the project name (defaults to directory name)
- `--force`: Overwrite existing files if they exist
- Positional argument: Target directory path (defaults to current directory)

**Interactive Setup:**

The init command will prompt you with several questions to customize your project:

1. **Project Type Selection:**
   ```
   ? What type of workflow would you like to create?
   ❯ ast-grep with JavaScript/TypeScript rules
     ast-grep with YAML rules  
     Blank workflow (custom commands)
   ```

2. **Language Selection** (if ast-grep is chosen):
   ```
   ? Which language would you like to target?
   ❯ JavaScript/TypeScript
     Python
     Rust
     Go
     Java
     C/C++
     Other
   ```

3. **Project Configuration:**
   ```
   ? Project name: my-codemod
   ? Description: Converts legacy API calls to new format
   ? Author: Your Name <your.email@example.com>
   ```

**Generated Project Structure:**

Depending on your selections, the init command creates different project templates:

#### ast-grep JavaScript Project
```
my-codemod/
├── workflow.yaml           # Main workflow definition
├── package.json           # Node.js dependencies
├── rules/
│   └── example-rule.js    # JavaScript AST transformation rules
├── scripts/
│   └── apply-codemod.js   # Script to apply transformations
├── tests/
│   ├── input/             # Test input files
│   └── expected/          # Expected output files
└── README.md              # Project documentation
```

#### ast-grep YAML Project
```
my-codemod/
├── workflow.yaml          # Main workflow definition
├── rules/
│   └── example-rule.yml   # YAML AST pattern rules
├── scripts/
│   └── apply-rules.sh     # Shell script to apply rules
├── tests/
│   ├── input/             # Test input files
│   └── expected/          # Expected output files
└── README.md              # Project documentation
```

#### Blank Workflow Project
```
my-workflow/
├── workflow.yaml          # Basic workflow template
├── scripts/
│   └── example.sh         # Example script
└── README.md              # Project documentation
```

**Example Usage:**

```bash
# Create a new ast-grep JavaScript project
$ codemod init my-js-codemod
? What type of workflow would you like to create? ast-grep with JavaScript/TypeScript rules
? Which language would you like to target? JavaScript/TypeScript
? Project name: my-js-codemod
? Description: Migrate from React class components to hooks
? Author: John Doe <john@example.com>

✓ Created workflow.yaml
✓ Created package.json with @ast-grep/cli dependency
✓ Created rules/migrate-to-hooks.js
✓ Created scripts/apply-codemod.js
✓ Created test structure
✓ Created README.md

Next steps:
  cd my-js-codemod
  npm install
  codemod validate -w workflow.yaml
  codemod run -w workflow.yaml
```

### `codemod run`

Execute a workflow from various sources:

```bash
# Run from a specific workflow file
codemod run -w workflow.yaml
codemod run -w path/to/workflow.yaml

# Run from a workflow bundle directory (containing workflow.yaml)
codemod run ./my-workflow-bundle/

# Run from a registry (Conceptual)
# codemod run my-registry/react-19-codemods:latest 
```

**Options:**
- `-w, --workflow <FILE>`: Path to the workflow definition file
- Positional argument: Path to workflow file or bundle directory

### `codemod resume`

Resume a paused workflow or trigger manual tasks:

```bash
# Resume a workflow run by ID
codemod resume -i <workflow-run-id>

# Trigger a specific task by UUID
codemod resume -i <workflow-run-id> -t <task-uuid>

# Trigger all tasks currently awaiting manual triggers
codemod resume -i <workflow-run-id> --trigger-all
```

**Options:**
- `-i, --id <UUID>`: Workflow run ID to resume
- `-t, --task <UUID>`: Specific task UUID to trigger
- `--trigger-all`: Trigger all tasks in `AwaitingTrigger` state

### `codemod validate`

Validate a workflow definition without executing it:

```bash
# Validate a workflow file
codemod validate -w workflow.yaml
codemod validate -w path/to/workflow.yaml

# Validate a workflow bundle
codemod validate ./my-workflow-bundle/
```

**Validation Checks:**
- Schema validation
- Node ID uniqueness
- Step ID uniqueness within nodes  
- Template ID uniqueness
- Dependency validation
- Cyclic dependency detection
- Template reference validation
- Matrix strategy validation
- State schema validation
- Variable reference syntax validation

**Example Output:**

Valid workflow:
```bash
$ codemod validate -w valid-workflow.yaml
✓ Workflow definition is valid
Schema validation: Passed
Node dependencies: Valid (3 nodes, 2 dependency relationships)
Template references: Valid (2 templates, 3 references)
Matrix strategies: Valid (1 matrix node)
State schema: Valid (2 schema definitions)
```

Invalid workflow:
```bash
$ codemod validate -w invalid-workflow.yaml
✗ Workflow definition is invalid
Error at nodes[2].strategy: Matrix strategy requires 'values' or 'from_state' property
Error at nodes[1].depends_on[0]: Referenced node 'non-existent-node' does not exist
Error: Cyclic dependency detected: node-a → node-b → node-a
```

### `codemod login`

Authenticate with a Butterflow registry to publish and manage codemods:

```bash
# Login to the default registry
codemod login

# Login to a specific registry
codemod login --registry https://registry.example.com

# Login with a token (for CI/CD)
codemod login --token <your-token>

# Login with username/password
codemod login --username <username>
```

**Options:**
- `--registry <URL>`: Registry URL (defaults to official Butterflow registry)
- `--token <TOKEN>`: Authentication token (for non-interactive login)
- `--username <USERNAME>`: Username for interactive login
- `--scope <SCOPE>`: Organization or user scope for publishing

**Interactive Login:**

```bash
$ codemod login
? Registry URL: https://app.butterflow.com(default)
? Username: john.doe
? Password: ********
? Organization (optional): my-org
✓ Successfully logged in as john.doe
✓ Default publish scope set to @my-org
```

**Token-based Login (for CI/CD):**

```bash
# Set authentication token
export BUTTERFLOW_TOKEN="your-registry-token"
codemod login --token $BUTTERFLOW_TOKEN
```

### `codemod publish`

Publish a codemod to a registry for sharing and reuse:

```bash
# Publish current directory as a codemod
codemod publish

# Publish a specific codemod directory
codemod publish ./my-codemod/

# Publish with specific version
codemod publish --version 1.2.3

# Publish to specific registry
codemod publish --registry https://registry.example.com

# Dry run (validate without publishing)
codemod publish --dry-run
```

**Options:**
- `--version <VERSION>`: Explicit version (overrides manifest version)
- `--registry <URL>`: Target registry URL
- `--tag <TAG>`: Tag for the release (e.g., `beta`, `latest`)
- `--access <LEVEL>`: Access level (`public` or `private`)
- `--dry-run`: Validate and pack without uploading
- `--force`: Override existing version (use with caution)

**Publishing Flow:**

```bash
$ codemod publish
✓ Validating codemod.yaml manifest
✓ Validating workflow.yaml
✓ Running tests (if present)
✓ Building codemod bundle
✓ Uploading to registry @my-org/react-hooks-migration@1.0.0
✓ Published successfully!

Install with: codemod run @my-org/react-hooks-migration@1.0.0
```

## Codemod Manifest Standard

Published codemods must include a `codemod.yaml` manifest file that defines metadata, dependencies, and publishing information.

### Manifest Schema

```yaml
# codemod.yaml
name: react-hooks-migration
version: 1.0.0
description: Migrates React class components to functional components with hooks
author: John Doe <john@example.com>
license: MIT
repository: https://github.com/user/react-hooks-migration

registry:
  access: public

targets:
  languages:
    - javascript
    - typescript
  frameworks:
    - react
  versions:
    react: ">=16.8.0"
```

### Required Fields

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Codemod package name (must be unique in scope) |
| `version` | string | Semantic version (e.g., "1.0.0") |
| `description` | string | Brief description of what the codemod does |
| `author` | string | Author name and email |

### Optional Fields

| Field | Type | Description |
|-------|------|-------------|
| `license` | string | License identifier (e.g., "MIT", "Apache-2.0") |
| `repository` | string | Repository URL for the codemod source code |
| `registry.access` | string | Access level: "public" or "private" |
| `targets.languages` | array | Supported programming languages |
| `targets.frameworks` | array | Supported frameworks or libraries |
| `targets.versions` | object | Version constraints for frameworks |

### Publishing Examples

#### Basic Codemod

```yaml
name: remove-console-logs
version: 1.0.0
description: Removes console.log statements from JavaScript/TypeScript files
author: Developer <dev@example.com>
license: MIT
repository: https://github.com/dev/remove-console-logs

targets:
  languages: [javascript, typescript]
```

#### Framework-Specific Codemod

```yaml
name: api-migration-v2
version: 2.1.0
description: Migrates legacy API calls to new v2 endpoints
author: API Team <api-team@company.com>
license: Apache-2.0
repository: https://github.com/company/api-migration-v2

registry:
  access: private

targets:
  languages: [javascript, typescript]
  frameworks: [react, vue, angular]
  versions:
    react: ">=16.0.0"
    vue: ">=3.0.0"
```

## Workflow Sources

The CLI supports loading workflows from different sources:

### File Path
```bash
codemod run -w path/to/your/workflow.yaml
```
Loads the specific file. The base path for execution is the directory containing this file.

### Directory Path (Bundle)
```bash
codemod run path/to/your/bundle/
```
Butterflow looks for a standard workflow file (e.g., `workflow.yaml`) inside this directory. The base path for execution is this directory.

### Registry Identifier
```bash
# Install and run a published codemod
codemod run @my-org/react-hooks-migration@1.0.0

# Run latest version
codemod run @my-org/react-hooks-migration@latest

# Run with custom parameters
codemod run @my-org/api-migration-v2@2.1.0 --param api_base_url=https://staging-api.example.com
```

When running from a registry, Butterflow:
1. Downloads the codemod bundle from the registry
2. Caches it locally for faster subsequent runs
3. Validates the manifest and workflow
4. Executes with the bundle directory as the base path

For your information the defautl registry is `https://app.codemod.com/`. and you can visualize codemods on the [Codemod Registry webapp](https://app.codemod.com/registry).

## Workflow Bundles

A workflow bundle is a directory containing:
1. The main workflow definition file (e.g., `workflow.yaml`)
2. Any scripts, binaries, or configuration files referenced by the workflow steps
3. Additional assets needed by the tasks

When running from a directory, Butterflow uses that directory as the root for resolving relative paths during execution.

## Runtime Execution

### Container Runtimes

The CLI supports multiple execution runtimes:

- **Docker**: Uses Docker daemon for container execution
- **Podman**: Uses Podman for container execution  
- **Direct**: Runs commands directly on the host machine

When using `runtime: direct`, commands execute with the same working directory as the `codemod` CLI invocation. Use the `$CODEMOD_PATH` environment variable to access files within the workflow bundle.

### Environment Variables

The CLI provides several environment variables to running tasks:

- `$STATE_OUTPUTS`: File descriptor for writing state updates
- `$CODEMOD_PATH`: Absolute path to the workflow bundle root (for `direct` runtime)
- `$BUTTERFLOW_STATE`: Path to state file for programmatic access

## Manual Triggers and Resumption

### Manual Nodes

Nodes with `type: manual` or `trigger: { type: manual }` will pause execution and await manual triggers:

```yaml
nodes:
  - id: deploy-prod
    name: Deploy to Production
    type: automatic
    trigger:
      type: manual  # Requires manual approval
    steps:
      - id: deploy
        run: ./deploy.sh production
```

### Triggering Manual Tasks

When a workflow pauses for manual triggers:

1. The workflow state is persisted with tasks marked `AwaitingTrigger`
2. The CLI provides task UUIDs for manual triggering
3. Use `codemod resume` to trigger specific tasks or all awaiting tasks

```bash
# Trigger specific deployment task
codemod resume -i abc123-workflow-run-id -t def456-task-uuid

# Trigger all awaiting tasks and continue
codemod resume -i abc123-workflow-run-id --trigger-all
```

## Error Handling

### Automatic Validation

Validation is performed automatically when running workflows:

```bash
$ codemod run -w invalid-workflow.yaml
✗ Workflow definition is invalid
Error: Cyclic dependency detected: node-a → node-b → node-a
Workflow execution aborted
```

### Resume After Failures

If a workflow fails or is interrupted:

1. The state is automatically persisted
2. Use `codemod resume` to continue from the last checkpoint
3. Failed tasks can be retried while preserving completed work

## Examples

### Basic Workflow Execution

```bash
# Create and run a simple workflow
cat > hello.yaml << EOF
version: "1"
nodes:
  - id: greet
    name: Greeting
    type: automatic
    runtime:
      type: direct
    steps:
      - id: hello
        run: echo "Hello from Butterflow!"
EOF

codemod run -w hello.yaml
```

### Matrix Workflow with Manual Approval

```bash
# Run matrix workflow requiring manual triggers
codemod run -w deploy-workflow.yaml

# When paused, trigger specific environments
codemod resume -i <run-id> -t <staging-task-uuid>
codemod resume -i <run-id> -t <prod-task-uuid>
```

### Workflow Validation and Publishing

```bash
# Validate before running
codemod validate -w complex-workflow.yaml

# Run if validation passes
codemod run -w complex-workflow.yaml

# Login to registry and publish a codemod
codemod login
codemod publish ./my-codemod/

# Run published codemod
codemod run @my-org/my-codemod@1.0.0
```

### Publishing Workflow

```bash
# Create and publish a new codemod
codemod init my-api-migration
cd my-api-migration

# Develop and test your codemod
# ... edit workflow.yaml, add rules, scripts, etc.

# Create manifest
cat > codemod.yaml << EOF
name: my-api-migration  
version: 1.0.0
description: Migrates legacy API calls
author: Developer <dev@example.com>
workflow: workflow.yaml
targets:
  languages: [javascript, typescript]
EOF

# Validate and publish
codemod validate
codemod publish --dry-run  # Preview
codemod publish           # Publish to registry
```

## Global Options

Most commands support these global options:

- `--help, -h`: Show help information
- `--version, -V`: Show version information
- `--verbose, -v`: Enable verbose output
- `--quiet, -q`: Suppress non-essential output

For detailed help on any command, use:

```bash
codemod <command> --help
```

## jssg (JavaScript ast-grep)

jssg is a JavaScript/TypeScript codemod runner and testing framework inspired by [ast-grep](https://ast-grep.github.io/). It enables you to write codemods in JavaScript and apply them to codebases with powerful CLI and test automation support.

### Running a Codemod

To run a JavaScript codemod on a target directory:

```bash
codemod jssg run my-codemod.js ./src --language javascript
```

**Options:**
- `--language <LANG>`: Target language (javascript, typescript, etc.)
- `--extensions <ext1,ext2>`: Comma-separated list of file extensions to process
- `--no-gitignore`: Do not respect .gitignore files
- `--include-hidden`: Include hidden files and directories
- `--max-threads <N>`: Maximum number of concurrent threads
- `--dry-run`: Perform a dry run without making changes

See `codemod jssg run --help` for all options.

### Example

```bash
codemod jssg run my-codemod.js ./src --language javascript --dry-run
```

---

# jssg Testing Framework Usage Guide

## Overview

The jssg Testing Framework provides comprehensive testing capabilities for JavaScript codemods with before/after fixture files. It integrates with the existing ExecutionEngine and provides a familiar test runner interface.

## Quick Start

### 1. Basic Test Structure

Create a test directory with the following structure:

```
tests/
├── simple-transform/
│   ├── input.js
│   └── expected.js
├── complex-case/
│   ├── input.ts
│   └── expected.ts
└── multi-file/
    ├── input/
    │   ├── file1.js
    │   └── file2.js
    └── expected/
        ├── file1.js
        └── file2.js
```

### 2. Running Tests

```bash
# Basic test run (language can be specified in config file)
codemod jssg test my-codemod.js --language javascript

# With custom test directory
codemod jssg test my-codemod.js --language typescript --test-directory ./my-tests

# Update snapshots (create/update expected files)
codemod jssg test my-codemod.js --language javascript --update-snapshots

# Verbose output with detailed diffs
codemod jssg test my-codemod.js --language javascript --verbose

# Watch mode (re-run tests on file changes)
codemod jssg test my-codemod.js --language javascript --watch

# Using configuration files (no CLI args needed)
codemod jssg test my-codemod.js
```

## CLI Options

### Required Arguments
- `codemod_file`: Path to the codemod JavaScript file

### Optional Arguments
- `--language`: Target language (javascript, typescript, etc.) - can be specified in config file

### Test Discovery
- `--test-directory`: Test directory (default: "tests")
- `--filter`: Run only tests matching pattern

### Output Control
- `--reporter`: Output format (console, json, terse)
- `--verbose`: Show detailed output
- `--context-lines`: Number of diff context lines (default: 3)
- `--ignore-whitespace`: Ignore whitespace in comparisons

### Test Execution
- `--timeout`: Test timeout in seconds (default: 30)
- `--max-threads`: Maximum concurrent threads
- `--sequential`: Run tests sequentially
- `--fail-fast`: Stop on first failure

### Snapshot Management
- `--update-snapshots`: Create/update expected files
- `--expect-errors`: Comma-separated patterns for tests expected to fail

### Development
- `--watch`: Watch for changes and re-run tests

## Test File Formats

### Single File Format
Each test case is a directory with `input.{ext}` and `expected.{ext}` files:

```
test-case-name/
├── input.js      # Input code
└── expected.js   # Expected output
```

### Multi-File Format
For testing multiple files, use `input/` and `expected/` directories:

```
test-case-name/
├── input/
│   ├── file1.js
│   └── file2.js
└── expected/
    ├── file1.js
    └── file2.js
```

## Language Support

The framework automatically detects input files based on language extensions:

- **JavaScript**: `.js`, `.jsx`, `.mjs`
- **TypeScript**: `.ts`, `.tsx`, `.mts`
- **Other languages**: Determined by `get_extensions_for_language()`

## Error Handling

### Missing Expected Files
```bash
# Error: No expected file found
codemod jssg test my-codemod.js --language javascript
# Error: No expected file found for input.js in tests/my-test. Run with --update-snapshots to create it.

# Solution: Create expected files
codemod jssg test my-codemod.js --language javascript --update-snapshots
# Created expected file for my-test/input.js
```

### Ambiguous Input Files
```bash
# Error: Multiple input files found
# tests/my-test/input.js and tests/my-test/input.ts both exist

# Solution: Use only one input file or organize into directories
```

### Expected Test Failures
```bash
# Test cases that should fail
codemod jssg test my-codemod.js --language javascript --expect-errors "error-case,invalid-syntax"
```

## Output Formats

### Console (Default)
```
test my-test ... ok
test failing-test ... FAILED

failures:

---- failing-test stdout ----
Output mismatch for file expected.js:
-const x = 1;
+const y = 1;

test result: FAILED. 1 passed; 1 failed; 0 ignored
```

### JSON
```bash
codemod jssg test my-codemod.js --language javascript --reporter json
```

```json
{
  "type": "suite",
  "event": "started",
  "test_count": 2
}
{
  "type": "test",
  "event": "started",
  "name": "my-test"
}
{
  "type": "test",
  "name": "my-test",
  "event": "ok"
}
```

### Terse
```bash
codemod jssg test my-codemod.js --language javascript --reporter terse
```

```
..F

failures:
...
```

## Advanced Features

### Snapshot Management
```bash
# Create initial snapshots
codemod jssg test my-codemod.js --language javascript --update-snapshots

# Update specific test snapshots
codemod jssg test my-codemod.js --language javascript --filter "specific-test" --update-snapshots
```

### Test Filtering
```bash
# Run tests matching pattern
codemod jssg test my-codemod.js --language javascript --filter "transform"

# Run specific test
codemod jssg test my-codemod.js --language javascript --filter "my-specific-test"
```

### Performance Tuning
```bash
# Limit concurrent threads
codemod jssg test my-codemod.js --language javascript --max-threads 2

# Run sequentially for debugging
codemod jssg test my-codemod.js --language javascript --sequential

# Set custom timeout
codemod jssg test my-codemod.js --language javascript --timeout 60
```

### Diff Customization
```bash
# More context in diffs
codemod jssg test my-codemod.js --language javascript --context-lines 5

# Ignore whitespace differences
codemod jssg test my-codemod.js --language javascript --ignore-whitespace
```

## Integration with Development Workflow

### CI/CD Integration
```yaml
# GitHub Actions example
- name: Run codemod tests
  run: codemod jssg test my-codemod.js --language javascript --reporter json
```

### IDE Integration
The framework outputs standard test results that can be consumed by IDEs and test runners.

### Watch Mode Development
```bash
# Continuous testing during development
codemod jssg test my-codemod.js --language javascript --watch --verbose
```

## Best Practices

1. **Organize tests by functionality**: Group related test cases in descriptive directories
2. **Use meaningful test names**: Directory names become test names in output
3. **Start with --update-snapshots**: Generate initial expected files, then review and commit
4. **Use --expect-errors for negative tests**: Test error conditions explicitly
5. **Leverage --filter during development**: Focus on specific tests while developing
6. **Use --watch for rapid iteration**: Get immediate feedback on changes

## Troubleshooting

### Common Issues

1. **No tests found**: Check test directory path and file extensions
2. **Ambiguous input files**: Ensure only one input file per test case
3. **Timeout errors**: Increase timeout for complex codemods
4. **Memory issues**: Reduce max-threads for large test suites

### Debug Mode
```bash
# Verbose output for debugging
codemod jssg test my-codemod.js --language javascript --verbose --sequential
```

This framework provides a robust foundation for testing JavaScript codemods with familiar tooling and comprehensive features.

## Configuration System

jssg supports a hierarchical configuration system using configuration files, allowing you to specify test settings globally and override them per test case. Multiple file formats and aliases are supported.

### Configuration Files

#### Supported Formats and Names

Configuration files are searched in the following order of precedence (first found wins):

1. `test.config.json` - JSON format
2. `test.config.yaml` - YAML format  
3. `codemod-test.config.json` - JSON format with codemod prefix
4. `codemod-test.config.yaml` - YAML format with codemod prefix

#### Global Configuration

Create a configuration file in your project root to set default configurations:

**JSON format (`test.config.json`):**
```json
{
  "language": "typescript",
  "timeout": 30,
  "ignore_whitespace": false
}
```

**YAML format (`test.config.yaml`):**
```yaml
language: typescript
timeout: 30
ignore_whitespace: false
```

#### Test Case Configuration

Create configuration files in individual test case directories to override global settings:

```
tests/
├── test.config.yaml          # Global config (YAML)
├── simple-transform/
│   ├── input.js
│   ├── expected.js
│   └── test.config.json      # Test-specific config (JSON)
└── complex-case/
    ├── input.ts
    ├── expected.ts
    └── codemod-test.config.yaml  # Different test-specific config (YAML with prefix)
```

**Example test-specific configs:**

JSON format (`test.config.json`):
```json
{
  "language": "javascript",
  "expect_errors": ["syntax-error"],
  "timeout": 60
}
```

YAML format (`test.config.yaml`):
```yaml
language: javascript
expect_errors:
  - syntax-error
timeout: 60
```

### Configuration Schema

Configuration options that can be specified in configuration files:

**Per-Test Configurable Options:**
| Option | Type | Description |
|--------|------|-------------|
| `language` | string | Target language (javascript, typescript, etc.) |
| `timeout` | number | Test timeout in seconds |
| `ignore_whitespace` | boolean | Ignore whitespace differences in comparisons |
| `expect_errors` | string[] | Test patterns expected to produce errors |

**Global-Only Options (CLI arguments only):**
These options control test execution behavior and cannot be configured per-test:
- `test_directory` - Directory containing test fixtures
- `filter` - Run only tests matching this pattern  
- `update_snapshots` - Update expected outputs with actual results
- `verbose` - Show detailed output for each test
- `sequential` - Run tests sequentially instead of in parallel
- `max_threads` - Maximum number of concurrent test threads
- `fail_fast` - Stop on first test failure
- `watch` - Watch for file changes and re-run tests
- `reporter` - Output format (console, json, terse)
- `context_lines` - Number of context lines in diff output

### Configuration Precedence

Configuration is resolved differently for per-test and global options:

**For Per-Test Options** (language, timeout, ignore_whitespace, expect_errors):
1. Default values
2. Root `test.config.json` (walking up directory tree)
3. Parent directory configs (inheriting from root to test case)
4. Test case `test.config.json` (highest priority for per-test options)
5. CLI arguments (override everything)

**For Global Options** (all execution behavior settings):
- Only CLI arguments are considered - config files are ignored for these options

### Usage Examples

#### Basic Global Configuration

**Using JSON:**
```bash
# Create global config (only per-test options)
cat > test.config.json << EOF
{
  "language": "typescript",
  "timeout": 60
}
EOF
```

**Using YAML:**
```bash
# Create global config (only per-test options)
cat > test.config.yaml << EOF
language: typescript
timeout: 60
EOF
```

**Running tests:**
```bash
# Run tests (use CLI for global execution options)
codemod jssg test my-codemod.js --reporter json --max-threads 2
```

#### Per-Test-Case Language Support

```bash
# Global config (YAML)
cat > test.config.yaml << EOF
language: javascript
EOF

# TypeScript-specific test (JSON)
mkdir tests/typescript-test
cat > tests/typescript-test/test.config.json << EOF
{
  "language": "typescript"
}
EOF

# Another test with codemod prefix (YAML)
mkdir tests/special-test  
cat > tests/special-test/codemod-test.config.yaml << EOF
language: typescript
timeout: 120
expect_errors:
  - compile-error
EOF

# Mixed language and format testing works automatically
codemod jssg test my-codemod.js
```

#### Development Workflow

```bash
# Set up development-friendly defaults (per-test options only)
cat > test.config.json << EOF
{
  "language": "typescript",
  "timeout": 120
}
EOF

# Use CLI args for execution behavior
# Development
codemod jssg test my-codemod.js --verbose --fail-fast --context-lines 5

# CI
codemod jssg test my-codemod.js --reporter json --max-threads 1 --sequential
```

### Configuration Discovery

The system automatically discovers per-test configuration files by:

1. Starting from the current working directory
2. Looking for configuration files in the supported formats and names
3. Walking up the directory tree to find parent configs
4. For each test case, checking for test-specific configuration files
5. Merging configurations with proper inheritance (child overrides parent)
6. Applying CLI argument overrides (for both global and per-test options)

**Configuration File Discovery Order:**
At each directory level, the system checks for files in this order:
1. `test.config.json`
2. `test.config.yaml`
3. `codemod-test.config.json`
4. `codemod-test.config.yaml`

The first file found is used for that directory level.

This allows for flexible project organization where you can have:
- Repository-wide defaults in the root configuration file
- Module-specific configs in subdirectories (any supported format)
- Test-case specific overrides in individual test folders (any supported format)

**Important:** Only per-test options (language, timeout, ignore_whitespace, expect_errors) are inherited from config files. Global execution options must be specified via CLI arguments.

**Current Limitations:**
- Per-test timeout and ignore_whitespace settings are applied during codemod execution but not yet integrated with the test runner's timeout mechanism
- Per-test expect_errors patterns work correctly for individual test cases
