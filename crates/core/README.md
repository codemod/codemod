# Butterflow: A Self-Hostable Workflow Engine

Butterflow is a lightweight, self-hostable workflow engine designed for running large-scale code transformation jobs. Similar to GitHub Actions but with a focus on local execution and code transformations like codemods and grep operations.

## Installation

### Building from Source

```bash
# Clone the repository
git clone https://github.com/codemod/butterflow.git
cd butterflow

# Build the project
cargo build --release

# The executable will be available at target/release/butterflow
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
# Option 1: Run from a specific workflow file
butterflow run -w workflow.yaml

# Option 2: Run from a workflow bundle directory (containing workflow.yaml)
# (Assuming workflow.yaml is inside the 'my-workflow-bundle' directory)
butterflow run ./my-workflow-bundle/ 

# Option 3: Run from a registry (Conceptual)
# butterflow run my-registry/react-19-codemods:latest 
```

## Core Features

- **Self-hostable**: Run workflows on your own infrastructure
- **Local execution**: Execute workflows on your local machine
- **Lightweight**: Minimal resource requirements
- **Parallel execution**: Run nodes concurrently when dependencies allow
- **Manual triggers**: Some nodes can be manually triggered
- **Matrix execution**: Run the same node multiple times with different inputs, dynamically generated from state
- **OCI container support**: Each task runs in its own container (Optional, can also use direct execution or built-in tools)
- **Durable execution**: Workflow state is persisted with diff-based updates and can survive restarts
- **Flexible configuration**: Define workflows in either JSON or YAML format
- **Backend-agnostic state management**: Support for multiple state backends (local, API, database)
- **Resumable workflows**: Resume workflow execution from the last saved state
- **Reusable templates**: Define reusable workflow components
- **State schema definition**: Define the structure of the global shared workflow state
- **Global Shared State**: Centralized, schema-validated state for all tasks
- **Dynamic Task Recompilation**: Matrix tasks adapt on-the-fly to state changes
- **Flexible Workflow Sources**: Run workflows from single YAML/JSON files, local directories (bundles), or fetch bundles from a registry
- **Built-in AST grep**: Integrated ast-grep support with automatic language extension inference and fix application
- **Cycle detection for codemod dependencies**: Robust cycle detection to prevent infinite recursion when codemods reference each other

## Architecture

Butterflow consists of several components:

1. **Workflow Engine**: Orchestrates the execution of workflows based on the shared state and dependencies
2. **Task Runner**: Executes individual tasks using container runtimes
3. **State Manager**: Persists the global shared workflow state for durability with support for multiple backends
4. **Scheduler**: Determines which tasks can be executed based on dependencies and state changes
5. **State Synchronizer**: Manages state updates using diff-based synchronization
6. **Template Manager**: Manages reusable workflow templates

## Workflow Definition

Workflows are defined in a YAML or JSON file (typically named `workflow.yaml` or `butterflow.json`) which describes the nodes, steps, dependencies, and shared state schema.

### Workflow Bundles

A workflow isn't just the definition file; it often includes scripts, configuration files, rules, or other assets needed by the tasks. A **Workflow Bundle** is typically a directory containing:

1.  The main workflow definition file (e.g., `workflow.yaml`).
2.  Any scripts, binaries, or configuration files referenced by the workflow steps (e.g., `scripts/my-codemod.js`, `rules/lint-rules.txt`).

When you run Butterflow pointing to a directory, it will look for a default workflow definition file (like `workflow.yaml`) inside that directory and use the directory itself as the root for resolving relative paths during execution.

### Loading Workflows

Butterflow can load workflows from different sources:

-   **File Path**: `butterflow run -w path/to/your/workflow.yaml` or `butterflow run path/to/your/workflow.yaml` (if positional arguments are supported). Loads the specific file. The base path for execution is the directory containing this file.
-   **Directory Path**: `butterflow run path/to/your/bundle/`. Butterflow looks for a standard workflow file (e.g., `workflow.yaml`) inside this directory. The base path for execution is this directory.
-   **Registry Identifier (Conceptual)**: `butterflow run registry-alias/bundle-name:version`. Butterflow would fetch the corresponding bundle (e.g., a `.tgz` archive) from the configured registry, unpack it locally (e.g., into a cache), and run the workflow found inside. The base path for execution is the unpacked bundle directory.

### YAML Format

```yaml
version: "1"

state:
  schema:
    - name: i18nShardsTs
      type: array
      items:
        type: object
        properties:
          team:
            type: string
          shardId:
            type: string

templates:
  - id: checkout-repo
    name: Checkout Repository
    description: Standard process for checking out a repository
    runtime:
      type: docker
      image: alpine/git:latest
    inputs:
      - name: repo_url
        type: string
        required: true
        description: "URL of the repository to checkout"
        default: ${{params.repo_url}}
    steps:
      - id: clone
        name: Clone repository
        run: git clone ${{inputs.repo_url}} repo

nodes:
  - id: checkout-and-analyze
    name: Checkout and analyze code
    description: Clone repository and perform initial code analysis
    type: automatic
    runtime:
      type: docker
      image: node:18-alpine
    steps:
      - id: checkout-repo
        name: Checkout repo
        uses:
          - template: checkout-repo
            inputs:
              repo_url: ${{params.repo_url}}
      
      - id: find-console-logs
        name: Find console.log statements
        ast-grep:
          config_file: "rules/console-log.yaml"
          exclude:
            - "**/node_modules/**"
            - "**/*.test.*"
      
      - id: check-typescript-issues
        name: Check TypeScript code quality
        ast-grep:
          config_file: "rules/typescript-quality.yaml"
          base_path: "./repo"
          # Automatically searches .ts, .tsx files based on rule languages

  - id: evaluate-codeowners
    name: Evaluate codeowners
    description: Shard the Codemod run into smaller chunks based on the codeowners
    type: automatic
    depends_on:
      - checkout-and-analyze
    runtime:
      type: docker
      image: node:18-alpine
    steps:
      # Example step that might write to state
      - id: generate-shards
        name: Generate Shards
        run: |
          echo 'i18nShardsTs=[{"team":"frontend","shardId":"1"},{"team":"backend","shardId":"2"}]' >> $STATE_OUTPUTS

  - id: run-codemod-ts
    name: I18n Codemod (TS)
    description: Run the i18n codemod on the TS files based on generated shards
    type: automatic
    trigger:
      type: manual # Example: can be triggered manually
    depends_on:
      - evaluate-codeowners
    strategy:
      type: matrix
      from_state: i18nShardsTs # Dynamically generates tasks based on state
    steps:
      # Matrix variables 'team' and 'shardId' are injected from the state item
      - id: apply-i18n-codemod
        name: Apply i18n transformations
        ast-grep:
          config_file: "codemods/i18n-transform.yaml"
          base_path: "./repo"
          include:
            - "src/**/*.ts"
            - "src/**/*.tsx"
          exclude:
            - "**/*.test.*"
            - "**/*.spec.*"
      
      - id: verify-changes
        name: Verify codemod results
        run: echo "Applied i18n codemod for team $team on shard $shardId"
```

### Workflow Components

#### State Schema

The `state` section defines the schema for the global shared workflow state:

```yaml
state:
  schema:
    - name: stateName
      type: array|object|string|number|boolean
      items: # For array types
        type: object
        properties:
          property1:
            type: string
```

This schema ensures data consistency and validates updates made by tasks.

#### Templates

Templates are reusable workflow components:

```yaml
templates:
  - id: template-id
    name: Template Name
    description: Template description
    runtime:
      type: docker
      image: image:tag
    inputs:
      - name: input_name
        type: string|number|boolean
        required: true|false
        description: "Input description"
        default: default_value
    steps:
      - id: step-id
        name: Step name
        run: |
          command1
      - id: step-id-two
        name: Second step name
        run: |
          command2
```

#### Nodes

Nodes are the main execution units in a workflow:

```yaml
nodes:
  - id: node-id
    name: Node Name
    description: Node description
    type: automatic|manual
    trigger:
      type: manual|automatic
    depends_on:
      - other-node-id
    runtime:
      type: docker
      image: image:tag
    strategy:
      type: matrix
      from_state: stateName # Source matrix items from shared state
    steps:
      - id: step-id
        name: Step name
        uses:
          - template: template-id
            inputs:
              input_name: value
      - id: another-step
        name: Another step
        run: command1
    env:
      ENV_VAR: value
```

### Node Configuration

Each node in a workflow can have the following properties:

| Property      | Description                                                                |
| ------------- | -------------------------------------------------------------------------- |
| `id`          | Unique identifier for the node                                             |
| `name`        | Human-readable name                                                        |
| `description` | Detailed description of what the node does                                 |
| `type`        | Either "automatic" or "manual"                                             |
| `depends_on`  | Array of node IDs that must complete before this node can run              |
| `strategy`    | Configuration for running multiple instances of this node (e.g., `matrix`) |
| `trigger`     | Configuration for how the node is triggered                                |
| `runtime`     | Container runtime configuration                                            |
| `steps`       | Array of steps to execute sequentially within the node                     |
| `env`         | Environment variables to inject into the container                         |

### Step Configuration

Steps within a node can have the following properties:

| Property      | Description                                |
| ------------- | ------------------------------------------ |
| `id`          | Unique identifier for the step             |
| `name`        | Human-readable name                        |
| `description` | Detailed description of what the step does |
| `uses`        | Template to use for this step              |
| `run`         | Command to run                             |
| `ast-grep`    | AST grep configuration for code analysis   |

Steps can perform one of three types of actions:

1. **Template Usage** (`uses`): Execute a predefined template with specific inputs
2. **Script Execution** (`run`): Execute shell commands or scripts
3. **AST Grep Analysis** (`ast-grep`): Perform code searching and transformation using ast-grep

⚠️ Deprecated: Butterflow no longer supports per-step or per-node outputs. All state interactions are now done through the global shared state.

### AST Grep Integration

Butterflow includes built-in support for [ast-grep](https://ast-grep.github.io/), a fast code searching and rewriting tool. This integration provides powerful code analysis and transformation capabilities with automatic language detection and smart file filtering.

#### Basic AST Grep Step

```yaml
steps:
  - name: "Find console.log statements"
    ast-grep:
      config_file: "rules/console-log.yaml"
```

#### AST Grep Configuration

| Property      | Type                | Required | Description                                                                |
| ------------- | ------------------- | -------- | -------------------------------------------------------------------------- |
| `config_file` | `string`            | Yes      | Path to the ast-grep configuration file (.yaml or .json)                  |
| `include`     | `array<string>`     | No       | Include glob patterns (defaults to language-specific extensions)          |
| `exclude`     | `array<string>`     | No       | Exclude glob patterns                                                      |
| `base_path`   | `string`            | No       | Base path for resolving globs (defaults to current working directory)     |

#### Automatic Language Extension Inference

When the `include` field is omitted or empty, Butterflow automatically infers applicable file extensions based on the languages specified in your ast-grep rules:

```yaml
# ast-grep config file (rules.yaml)
id: console-log
language: javascript
rule:
  pattern: console.log($$$)
message: "Found console.log statement"
---
id: typescript-console
language: typescript  
rule:
  pattern: console.log($$$)
message: "Found console.log in TypeScript"
```

```yaml
# Workflow step - will automatically search *.js, *.mjs, *.cjs, *.ts, *.mts, *.cts files
steps:
  - name: "Find console.log in JS/TS files"
    ast-grep:
      config_file: "rules.yaml"
      # No include field - auto-infers from rule languages
```

## Node vs Task

In Butterflow:

- A **Node** is part of the workflow definition - it's a static component defined in your workflow YAML/JSON
- A **Task** is a runtime instance of a node that is executed - it's created dynamically during workflow execution, potentially multiple times if the node uses a matrix strategy or is re-evaluated based on state changes.

This distinction is particularly important in two scenarios:

1. **Matrix Strategies**: When a node uses a matrix strategy (especially `from_state`), multiple tasks are created dynamically based on the current value of the specified state key. Each task corresponds to an item in the matrix input, has its own unique UUID v4, status, and is associated with a specific hash of the input item.

2. **Manual Triggers**: When a node requires manual triggering (either because it's a manual node type or has a manual trigger configuration), the workflow engine creates the task(s) for that node, marks them as `AwaitingTrigger`, and then provides the specific task UUIDs to the user. This allows users to trigger individual tasks rather than entire nodes.

When a task is executed, it will run all the steps defined in its associated node. Tasks are always identified by UUID v4s to ensure uniqueness and proper tracking throughout the workflow execution.

### Task Creation and Management

Tasks are created or updated by the workflow engine as it determines what needs to be executed next based on dependencies and the current shared state. This is crucial for proper dependency tracking, dynamic adaptation, and execution planning:

- **Regular nodes**: A single task is usually created for each regular node when its dependencies are met.
- **Matrix nodes**: For matrix nodes using `from_state`:
  - The engine monitors the specified state key.
  - When the state changes, the engine re-evaluates the matrix input.
  - It calculates a stable hash for each item in the matrix input array/object.
  - Tasks are created for new items (based on hash).
  - Tasks whose corresponding input item hash is no longer present are marked `WontDo`.
  - Existing tasks whose input item hash remains are left untouched (unless their status changes, e.g., from `Pending` to `Running`).
  - A "master task" tracks the overall state of the matrix node, derived from the individual matrix item tasks.

The master task for a matrix node is not runnable itself; it exists solely to track the collective status of all matrix combination tasks.

If the matrix input state is not yet available (e.g., the state key is null or undefined), no tasks will be created for that node, and all dependencies of that node will be marked as blocked until the state becomes available.

## Dynamic Matrix Task Recompilation

When a node uses a `matrix` strategy and its input comes from the shared state (using `from_state`), Butterflow continuously watches the relevant state key. After **each task** finishes and potentially updates the state, Butterflow performs a re-evaluation cycle:

1.  **Recompute Matrix Inputs**: Reads the current value of the state key specified in `from_state` for each relevant matrix node.
2.  **Hash Items**: Calculates a stable, deep hash for each item in the retrieved state value (e.g., each object in an array). This hashing is order-independent for object keys.
3.  **Compare Hashes**: Compares the set of new hashes with the hashes associated with existing tasks for that node.
4.  **Add New Tasks**: If new hashes are found (items added to the state), new tasks are created, marked as `Pending`, and associated with these new hashes.
5.  **Mark Obsolete Tasks**: If hashes corresponding to previously existing tasks are no longer present (items removed from the state), those tasks are marked as `WontDo`.
6.  **Leave Unchanged Tasks**: Tasks corresponding to hashes that still exist in the state remain untouched (unless they were already `Completed` or `Failed`).

This means matrix tasks derived from state are **not static**. They are generated, updated, and potentially removed on the fly as the shared state evolves during the workflow run.

### Example

If the `i18nShardsTs` state key (defined in the schema as an array of objects) initially contains:

```json
[
  { "team": "frontend", "shardId": "1" },
  { "team": "backend", "shardId": "2" }
]
```

Then this node:

```yaml
- id: run-codemod-ts
  strategy:
    type: matrix
    from_state: i18nShardsTs
  steps:
    - id: run-codemod
      run: echo "Running TS codemod for team $team on shard $shardId"
```

...initially creates two tasks (one for frontend, one for backend), each associated with the hash of its corresponding object.

If a later task **appends** a new object `{"team": "shared", "shardId": "3"}` to the `i18nShardsTs` state array (e.g., using `i18nShardsTs@=... >> $STATE_OUTPUTS`), Butterflow will:

1. Detect the state change after the task completes.
2. Re-read `i18nShardsTs`.
3. Calculate the hash for the new `{"team": "shared", ...}` object.
4. See this hash is new.
5. Spawn a **third task** for the `shared` team, associated with the new hash.

Conversely, if an item were removed from the `i18nShardsTs` array, the task corresponding to the hash of that removed item would be marked as `WontDo`.

## Task Re-Evaluation and Scheduling Logic

Butterflow continuously determines which tasks can run based on a cycle of evaluation triggered after every task completion:

1.  **State Update Application**: The completed task's state diff (from `$STATE_OUTPUTS`) is parsed and applied to the local view of the global shared state.
2.  **State Persistence**: This diff is sent to the configured state persistence backend (local, API, etc.).
3.  **Dependency Check**: The engine re-evaluates the dependencies for all nodes/tasks that are not yet completed.
4.  **Matrix Recompilation**: For every node with a `strategy: matrix` using `from_state`, the engine performs the "Dynamic Matrix Task Recompilation" described above (re-reading state, hashing, comparing, creating/marking tasks).
5.  **Scheduling**: The engine identifies tasks that meet all criteria to run:
    - All dependencies listed in `depends_on` are `Completed`.
    - If it's a matrix task, its corresponding state item hash exists.
    - The task's status is `Pending`.
    - The task's trigger conditions are met (e.g., it's `type: automatic` or it's `type: manual` and has been explicitly triggered).
6.  **Execution**: Eligible tasks are scheduled for execution by the Task Runner.

This re-evaluation cycle ensures that the workflow dynamically adapts to the evolving shared state. It continues iteratively until one of the following conditions is met:

- All tasks reach a terminal state (`Completed`, `Failed`, `WontDo`).
- The workflow is paused because all runnable tasks are blocked by dependencies or are awaiting manual triggers (`AwaitingTrigger`).

### Example: Matrix Node with Master Task

Consider a matrix node that processes different regions:

```yaml
nodes:
  - id: process-regions
    name: Process Regions
    type: automatic
    strategy:
      type: matrix
      values: # Example using static values, could also use from_state
        - region: us-east
        - region: us-west
        - region: eu-central
    steps:
      - id: process
        run: echo "Processing region $region"
```

When this workflow runs, it will:

1. Create a master task with a UUID v4 (e.g., `123e4567-e89b-12d3-a456-426614174000`)
2. Create three individual tasks with unique UUID v4s (e.g., `...4001`, `...4002`, `...4003`), one for each region.
3. Track the overall status via the master task.
4. Execute each individual task according to the workflow rules (potentially in parallel).

The master task's status is derived from its child tasks:

- If all child tasks are `Completed`, the master task is `Completed`.
- If any child task is `Failed`, the master task is `Failed`.
- If any child task is `WontDo` and others are `Completed`, the master task might be `Completed` (depending on exact logic) or a specific state indicating partial completion.
- If some child tasks are `Running` and none have `Failed`, the master task is `Running`.

## Workflow vs Workflow Run

It's important to distinguish between a workflow and a workflow run:

- A **Workflow** is the definition of the process - the YAML/JSON file that describes nodes, dependencies, state schema, and execution rules.
- A **Workflow Run** is a specific execution instance of a workflow - created when a user initiates the workflow, identified by a unique UUID v4.

Each workflow run manages its own instance of the **global shared state**.

### Key Differences

| Feature    | Workflow                  | Workflow Run                                    |
| ---------- | ------------------------- | ----------------------------------------------- |
| Nature     | Static definition         | Runtime instance with UUID v4 identifier        |
| Source     | Defined in YAML/JSON      | Created when workflow is executed               |
| Components | Contains node definitions | Contains task instances with UUID v4s           |
| State      | Defines state _schema_    | Holds the _actual_ shared state (JSON document) |

### State Management in Workflow Runs

Each workflow run has its own state instance that includes:

- A copy of the original workflow definition
- The **current value** of the global shared state (adhering to the schema)
- Current status of all tasks (including dynamically generated matrix tasks)
- All resolved variables and parameters for the run
- Execution history and logs

This state is tied specifically to the workflow run UUID. Multiple runs of the same workflow definition will each have their own independent shared state.

## Variable Resolution

Butterflow supports a powerful variable resolution system using the `${{...}}` syntax. This allows you to reference parameters, environment variables, and **shared state values** throughout your workflow definition.

### Variable Types

- **Parameters** (_TODO_): Accessed with `${{params.name}}` - values passed when starting the workflow run.
- **Environment Variables**: Accessed with `${{env.NAME}}` - environment variables available to the Butterflow engine itself.
- **State Values**: Accessed with `${{state.key.subkey}}` - values read directly from the **global shared state** JSON document.
- **Matrix Values**: In matrix tasks, the values from the specific matrix item that generated the task are **directly accessible as environment variables** (e.g., `$region`, `$shardId` in the examples above) within the `run` command. They are _not_ accessed via `${{...}}` syntax within the `run` script itself, but can be referenced in other fields like `name` or `env` using standard variable syntax if needed (e.g., `name: "Process $region"`).

### Variable Resolution Examples

```yaml
nodes:
  - id: example-node
    name: "Processing ${{params.repo_name}}"
    description: "Running on branch ${{params.branch}}. Current user: ${{state.currentUser.name}}"
    env:
      REPO_URL: "${{params.repo_url}}" # Parameter
      DEBUG: "${{env.CI}}" # Environment variable for Butterflow engine
      API_ENDPOINT: "${{state.config.apiEndpoint}}" # Value from shared state
    steps:
      - id: process
        name: Process Data
        run: |
          echo "Processing data for ${{params.repo_name}}"
          echo "API endpoint from state: $API_ENDPOINT" # Access env var set from state
          echo "Direct access to state value (less common in run): ${{state.config.someValue}}"
```

In matrix nodes, the matrix item values are injected as environment variables into the `run` script's execution context:

```yaml
nodes:
  - id: process-regions-from-state
    strategy:
      type: matrix
      from_state: regionConfigs # Assumes state.regionConfigs is like [{region: "us-east", zone: "z1"}, ...]
    steps:
      - id: process
        # 'region' and 'zone' are available as $region and $zone inside the script
        run: echo "Processing region $region in zone $zone"
```

### Variable Resolution Order

Variables are resolved in the following order when evaluating fields like `name`, `description`, `env`, etc. (before task execution):

1. Parameters passed to the workflow run (`${{params...}}`)
2. Environment variables available to Butterflow (`${{env...}}`)
3. Shared state values (`${{state...}}`)

For `run` commands within **matrix tasks**, the specific matrix item values are additionally available as environment variables (`$variable_name`).

If a `${{...}}` variable cannot be resolved, the workflow engine will either:

- Use a default value if specified (though default syntax isn't shown here)
- Fail with an error if the variable reference is invalid or the value doesn't exist.

## Global Shared State

Butterflow uses a **single, mutable, shared state** for the entire workflow run. This state is defined using the `state.schema` section in your workflow YAML file and acts as a structured JSON document. All tasks read from and can write to this central state, enabling complex coordination and dynamic behavior.

### Benefits of Shared State

- **Simple Mental Model**: Eliminates the need to track and pass outputs between individual steps or tasks. State is global and accessible.
- **Schema Validation**: The defined schema ensures that tasks write data in the expected format, preventing runtime type errors.
- **Dynamic Task Generation**: Enables powerful patterns like matrix strategies (`from_state`) where tasks are created based on live data within the state.
- **Centralized Coordination**: Tasks can react to changes made by other tasks by reading the shared state.
- **Advanced Operations**: Supports operations like appending to arrays or merging objects within the state via special syntax.

### Writing to State from Tasks

Tasks update the shared state by writing specially-formatted strings to a file descriptor provided via the environment variable `$STATE_OUTPUTS`. The engine monitors this file descriptor during task execution.

The syntax for updates is:

```
KEY=VALUE
KEY@=VALUE
```

| Syntax     | Meaning                                                                      | Example                                      | Notes                                          |
| :--------- | :--------------------------------------------------------------------------- | :------------------------------------------- | :--------------------------------------------- |
| `KEY=VAL`  | Sets the state key `KEY` to the JSON value `VAL`. Overwrites existing value. | `user={"name":"Alice","id":123}`             | `VAL` must be valid JSON.                      |
| `KEY=VAL`  | Sets the state key `KEY` to the primitive value `VAL`.                       | `count=10`                                   | `VAL` is treated as a string if not JSON-like. |
| `KEY@=VAL` | Appends the JSON value `VAL` to the array at state key `KEY`.                | `users@={"name":"Bob","id":456}`             | `KEY` must point to an array in the schema.    |
| `KEY@=VAL` | Appends the primitive value `VAL` to the array at state key `KEY`.           | `logMessages@="Task completed successfully"` | `KEY` must point to an array in the schema.    |

**Important**:

- `KEY` can use dot notation to access nested fields (e.g., `config.retries=5`).
- `VALUE` **must be valid JSON** if it represents an object or array (e.g., `{"key": "value"}` or `[1, 2]`). Primitive types like strings, numbers, and booleans can be written directly (e.g., `count=5`, `enabled=true`, `message="hello"`). String values containing spaces or special characters should ideally be quoted or formatted as a JSON string (e.g., `message=""Hello World""` or use a heredoc).
- Updates are collected by the engine and applied as a **diff** to the global state _after_ the task successfully completes.
- The engine uses the schema defined in `state.schema` to validate the type of `VALUE` being written.

#### Example Bash Script Step:

This step appends a new user object to the `users` array in the shared state.

```yaml
nodes:
  - id: add-user
    steps:
      - id: append-user
        name: Append User to State
        run: |
          # Construct a JSON string for the object
          NEW_USER_JSON='{"team":"frontend","shardId":"1"}'
          # Write the append command to the state file descriptor
          echo "i18nShardsTs@=$NEW_USER_JSON" >> $STATE_OUTPUTS
          echo "Processed user $NEW_USER_JSON" # Normal stdout for logs
```

#### Example Python Script Step:

```yaml
nodes:
  - id: update-count
    steps:
      - id: increment
        name: Increment Counter in State
        runtime:
          type: docker
          image: python:3.9-slim
        run: |
          #!/usr/bin/env python
          import os
          import json

          # Assume current count is read from somewhere or is known
          new_count = 11 
          state_file = os.environ['BUTTERFLOW_STATE']

          with open(state_file, 'a') as f:
              # Set a simple numeric value
              f.write(f"processedCount={new_count}\n") 
              # Append a log message (as a JSON string)
              f.write(f"logMessages@="Processed item {new_count}"\n") 

          print(f"Updated count to {new_count}") # Normal stdout
```

Butterflow automatically parses the content written to `$STATE_OUTPUTS`, validates it against the schema, and applies the changes as atomic diffs to the global shared state upon successful task completion. This change then triggers the re-evaluation cycle described earlier.

### Example: Workflow with Parameters

When running a workflow with parameters:

```bash
# ... existing code ...
```

### Example: Matrix Node with Manual Trigger

Consider a node with a matrix strategy that generates tasks dynamically from state, and requires manual triggering:

```yaml
state:
  schema:
    - name: deploymentTargets
      type: array
      items:
        type: object
        properties:
          env: { type: string }
          region: { type: string }

nodes:
  - id: prep-targets
    # ... steps that populate state.deploymentTargets ...
    run: |
      echo 'deploymentTargets=[{"env":"staging","region":"us-east"},{"env":"prod","region":"eu-west"}]' >> $STATE_OUTPUTS

  - id: deploy-app
    name: Deploy Application
    type: automatic # Node is automatic, but trigger is manual
    depends_on:
      - prep-targets
    trigger:
      type: manual # Requires manual trigger for each generated task
    strategy:
      type: matrix
      from_state: deploymentTargets # Tasks generated based on state
    steps:
      - id: deploy
        run: echo "Deploying to $env in region $region"
```

When this workflow runs:

1. The `prep-targets` node runs and populates `state.deploymentTargets`.
2. After `prep-targets` completes, the engine re-evaluates.
3. It sees `deploy-app` depends on `prep-targets` (completed) and uses `state.deploymentTargets` for its matrix.
4. It reads `state.deploymentTargets`, finds two items, and calculates their hashes.
5. It creates two distinct tasks for `deploy-app` (one for staging/us-east, one for prod/eu-west), each associated with its hash.
6. Because `trigger: manual` is set, both tasks are marked as `AwaitingTrigger`.
7. The engine provides the specific task UUIDs to the user.

The user can then choose to trigger individual deployment tasks using their UUIDs:

```bash
# Trigger only the staging deployment task
butterflow resume -i <workflow-run-id> -t <staging-task-uuid>
```

Or trigger all remaining tasks:

```bash
# Trigger both staging and prod tasks
butterflow resume -i <workflow-run-id> --trigger-all
```

This demonstrates how shared state, dynamic matrix generation, and manual triggers combine for fine-grained control over complex workflows.

### Task Status Tracking

During workflow execution, each task (including dynamically generated matrix tasks) can have one of the following statuses:

| Status            | Description                                                                                                                  |
| ----------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `Pending`         | Task hasn't started execution yet; dependencies met, awaiting runner capacity.                                               |
| `Running`         | Task is currently being executed by a runner.                                                                                |
| `Completed`       | Task finished successfully. State updates (if any) have been applied.                                                        |
| `Failed`          | Task execution failed with an error. State updates were not applied.                                                         |
| `AwaitingTrigger` | Task is waiting for a manual trigger via the `resume` command.                                                               |
| `Blocked`         | Task cannot run because one or more dependencies are not `Completed`.                                                        |
| `WontDo`          | Task will not be executed. Typically used for matrix tasks whose input item disappeared from the state during recompilation. |

The workflow engine tracks these statuses persistently in the state backend, allowing for:

- Resuming workflows after interruptions
- Visualizing workflow progress accurately, including dynamic tasks
- Identifying bottlenecks or failures
- Supporting partial re-runs of workflows where only failed or pending tasks are executed

For matrix nodes, the status of the master task reflects the aggregate status of its dynamically managed child tasks.

### Manual Nodes and Triggers

Butterflow supports two ways to introduce manual intervention points:

1.  **Manual Node Type**: A node defined with `type: manual` will always pause execution and create a task marked `AwaitingTrigger`, even if all dependencies are satisfied. This forces a user interaction point.

2.  **Manual Trigger**: A node (usually `type: automatic`) defined with `trigger: { type: manual }` will also create tasks marked `AwaitingTrigger`. This is useful for implementing approval gates within an otherwise automated flow, or for controlling the rollout of matrix tasks generated from state.

When a workflow encounters a task that becomes `AwaitingTrigger`:

1. The engine pauses execution _for that task and any downstream tasks depending on it_.
2. Other independent branches of the workflow continue to execute.
3. The workflow state (including the `AwaitingTrigger` status) is persisted.

Resuming a workflow with tasks awaiting triggers:

```bash
# Trigger a specific task using its UUID and resume
butterflow resume -i <workflow-run-id> -t <task-uuid>

# Trigger all tasks currently in AwaitingTrigger state for this run and resume
butterflow resume -i <workflow-run-id> --trigger-all
```

When resumed, the triggered task(s) will be scheduled for execution if their other dependencies are met, and the workflow continues from the persisted state.

## Container Execution (and Alternatives)

Each task runs in its own isolated environment. Butterflow supports multiple execution runtimes:

- **Docker**: Uses the Docker daemon installed on the host to run task steps in containers. Ideal for complex dependencies or ensuring a consistent environment.
- **Podman**: Uses Podman for container execution (if available).
- **Direct**: Runs commands directly on the host machine where Butterflow is running. **Important**: When using `runtime: direct`, the command is executed with the **same current working directory** as the `butterflow` CLI invocation. To access files within the workflow bundle, use the environment variable `$CODEMOD_PATH`, which Butterflow injects with the absolute path to the root of the bundle directory (e.g., `node "$CODEMOD_PATH/scripts/my-codemod.js"`). Use with caution regarding environment consistency and security.
- **Native (Conceptual)**: For tools tightly integrated with Butterflow (like `ast-grep` if built-in), this runtime could execute optimized Rust code directly, avoiding container/process overhead.

The runtime is typically configured per-node or per-template. Choosing `direct` simplifies deployment if users can manage the necessary host dependencies (like Node.js), while containers provide better isolation and dependency management at the cost of setup complexity (installing Docker/Podman).

## State Management

Butterflow implements a comprehensive backend-agnostic state management system centered around the **global shared state**.

Key features include:

- **Diff-based updates**: Tasks declare state changes via `$STATE_OUTPUTS`. The engine calculates the diff and applies it, minimizing data transfer and enabling efficient updates. Only changes are sent to the persistence backend.
- **Multiple backend support**: Store the workflow run state (including the shared state document, task statuses, etc.) in local files, databases, or remote APIs.
- **API integration**: Send state diffs to external systems via configurable API calls, with retry mechanisms for robustness.
- **Conflict resolution**: The diff-based approach inherently handles concurrent updates to _different_ parts of the state. Updates to the _same_ part are applied sequentially based on task completion order.
- **Transactional updates**: State changes from a task are applied atomically only upon successful completion of the task.
- **State versioning**: Backends can optionally track changes to workflow state over time.
- **Schema validation**: The shared state is validated against the `state.schema` on writes.
- **Local fallback**: API backends can be configured to fall back to local storage if the remote endpoint is temporarily unavailable.

The state management system tracks:

- The current JSON document representing the global shared state.
- Current status (`Pending`, `Running`, `Completed`, `Failed`, `AwaitingTrigger`, `WontDo`, `Blocked`) of each task, including matrix tasks.
- The association between matrix tasks and their input item hashes.
- Manual trigger status.
- Execution history and logs (may vary by backend).

### State Persistence

Workflow state is persisted automatically at critical points during execution:

- After each task completes and its state diff is applied.
- When a task is marked `AwaitingTrigger`.
- Periodically (configurable) as checkpoints.

This persistence allows for:

- **Resume after interruptions**: If Butterflow or the machine restarts, workflows can resume from the last saved state.
- **Failure recovery**: If a task fails, the workflow can potentially be retried or resumed, skipping already completed tasks.
- **Manual trigger handling**: State is saved reliably while waiting for manual triggers.
- **Efficient storage**: Diff-based updates minimize storage growth, especially for large states or long-running workflows.

### Transactional State Updates

Butterflow uses a diff-based approach for state updates. When a task completes, the engine:

1. Reads the updates specified via `$STATE_OUTPUTS`.
2. Validates them against the `state.schema`.
3. Calculates the resulting state diff (changes).
4. Applies the diff atomically to its in-memory representation of the global shared state for that workflow run.
5. Sends this diff to the configured state persistence adapter.

This approach provides several key benefits:

- **Concurrency**: Multiple tasks within the same workflow run can potentially execute concurrently. Their state updates are collected and applied sequentially as they complete, modifying the shared state.
- **Atomicity**: State changes from a single task are applied as one atomic unit upon success. If the task fails, its changes are discarded.
- **Reduced Network Traffic**: Only sending diffs to the backend is efficient (if using a remote backend).
- **Audit Trail**: The sequence of diffs provides a history of state changes (if the backend supports versioning).

For example, if Task A writes `count=1` and Task B (running concurrently) writes `status="processed"`:

1. Task A completes, diff `{"op": "replace", "path": "/count", "value": 1}` is applied and persisted.
2. Task B completes, diff `{"op": "replace", "path": "/status", "value": "processed"}` is applied and persisted.
3. The final state reflects both changes.

In the shared state model, tasks write structured updates to state. These updates are diffed, hashed (where relevant for matrix recompilation), and applied transactionally. Any change to state fields used by `from_state` in matrix strategies triggers task recompilation, ensuring workflows dynamically adapt to new conditions discovered or created during execution.

## Workflow Validation

Butterflow performs comprehensive validation of workflow definitions before execution to ensure they are well-formed and can be executed correctly. This validation happens automatically when a workflow is loaded, but can also be performed explicitly using the `validate` command.

### Validation Checks

The following validation checks are performed:

1. **Schema Validation**: Ensures the workflow definition adheres to the expected schema
2. **Node ID Uniqueness**: Verifies that all node IDs are unique
3. **Step ID Uniqueness**: Checks that step IDs within a node are unique
4. **Template ID Uniqueness**: Ensures all template IDs are unique
5. **Dependency Validation**: Verifies that all referenced dependencies exist
6. **Cyclic Dependency Detection**: Prevents circular dependencies between nodes
7. **Template Reference Validation**: Ensures all template references are valid
8. **Matrix Strategy Validation**: Verifies that matrix strategies have valid configurations
9. **State Schema Validation**: Ensures `state.schema` definitions are valid and checks consistency between `from_state` references and the schema.
10. **Variable Reference Validation**: Checks that `${{...}}` references use valid syntax (e.g., `${{state.key}}`, `${{params.key}}`, `${{env.KEY}}`). It does _not_ typically validate the existence of keys at validation time, as state is dynamic.

### Cyclic Dependency Prevention

One of the most important validation checks is the detection of cyclic dependencies. A cyclic dependency occurs when a node depends on itself, either directly or indirectly through other nodes. For example:

```yaml
nodes:
  - id: node-a
    depends_on:
      - node-b

  - id: node-b
    depends_on:
      - node-c

  - id: node-c
    depends_on:
      - node-a # Creates a cycle: node-a → node-b → node-c → node-a
```

Butterflow detects such cycles using a topological sorting algorithm and reports them as validation errors:

```bash
$ butterflow validate -w cyclic-workflow.yaml
✗ Workflow definition is invalid
Error: Cyclic dependency detected: node-a → node-b → node-c → node-a
```

### Dependency Graph Visualization

Butterflow can generate a visual representation of the workflow's dependency graph to help identify potential issues:

```bash
$ butterflow graph -w workflow.yaml -o workflow-graph.png
Generating dependency graph...
Graph saved to workflow-graph.png
```

This visualization can be helpful for understanding complex workflows and verifying that dependencies are correctly defined.

### Validation Examples

#### Valid Workflow

```bash
$ butterflow validate -w valid-workflow.yaml
✓ Workflow definition is valid
Schema validation: Passed
Node dependencies: Valid (3 nodes, 2 dependency relationships)
Template references: Valid (2 templates, 3 references)
Matrix strategies: Valid (1 matrix node)
State schema: Valid (2 schema definitions)
```

#### Invalid Workflow with Multiple Issues

```bash
$ butterflow validate -w invalid-workflow.yaml
✗ Workflow definition is invalid
Error at nodes[2].strategy: Matrix strategy requires 'values' or 'from_state' property
Error at nodes[1].depends_on[0]: Referenced node 'non-existent-node' does not exist
Error: Cyclic dependency detected: node-a → node-b → node-a
Error at nodes[0].steps[1].uses[0]: Referenced template 'non-existent-template' does not exist
Error at nodes[3].strategy.from_state: State key 'nonExistentStateKey' not found in schema
```

### Automatic Validation

Validation is performed automatically when running a workflow:

```bash
$ butterflow run -w invalid-workflow.yaml
✗ Workflow definition is invalid
Error: Cyclic dependency detected: node-a → node-b → node-a
Workflow execution aborted
```

This ensures that only valid workflows are executed, preventing potential runtime issues.

### Schema Validation

A workflow JSON schema is provided in the `schemas` directory for, e.g., LSP
checks.

## Cycle Detection

The engine provides robust cycle detection to prevent infinite recursion when codemods reference each other. This validation happens at two levels:

### 1. Pre-execution Validation
Before starting any workflow, the engine analyzes the complete dependency tree to detect cycles:

```yaml
# This would be detected as a cycle:
# Codemod A -> Codemod B -> Codemod A
```

### 2. Runtime Validation
During execution, additional checks prevent dynamic cycles that might not be visible during static analysis.

### Supported Cycle Types
- **Direct cycles**: A codemod that references itself
- **Two-step cycles**: A → B → A
- **Multi-step cycles**: A → B → C → D → A
- **Complex dependency chains**: Any circular reference in the dependency graph

### Error Messages
When a cycle is detected, you'll receive a detailed error message:

```
Codemod dependency cycle detected!
Cycle: @org/codemod-a → @org/codemod-b → @org/codemod-a
This would cause infinite recursion during execution.
Please review your codemod dependencies to remove the circular reference.
```

### Best Practices
1. **Design linear dependencies**: Structure your codemods so they have a clear dependency hierarchy
2. **Use composition over delegation**: Instead of having codemods call each other, create shared libraries
3. **Test dependency chains**: Use smaller, focused codemods that can be combined safely
4. **Version pinning**: Use specific versions in registry references to avoid unexpected dependency changes

## Examples

### Basic Codemod Step
```yaml
version: "1.0.0"
nodes:
  - id: "transform"
    name: "Apply Transformations"
    steps:
      - name: "Update imports"
        codemod:
          source: "@myorg/import-updater@2.1.0"
          args: ["--style", "es6"]
          
      - name: "Format code"
        codemod:
          source: "./local-formatters/prettier-config"
          env:
            PRETTIER_CONFIG: "./custom.config.js"
```

### Complex Workflow with Dependencies
```yaml
version: "1.0.0"
nodes:
  - id: "prepare"
    name: "Prepare Codebase"
    steps:
      - name: "Clean up legacy code"
        codemod:
          source: "@cleanup/legacy-remover@1.0.0"
          
  - id: "modernize"
    name: "Modernize Code"
    depends_on: ["prepare"]
    steps:
      - name: "Update to modern syntax"
        codemod:
          source: "@modernize/syntax-updater@3.0.0"
          working_dir: "./src"
          
      - name: "Update dependencies"
        codemod:
          source: "@deps/updater@latest"
          args: ["--mode", "safe"]
```

### Error Handling
The cycle detection system will prevent workflows like this from running:

```yaml
# ❌ This will fail with cycle detection error
version: "1.0.0"
nodes:
  - id: "problematic"
    steps:
      - name: "This creates a cycle"
        codemod:
          source: "@circular/codemod-a"  # Which internally uses @circular/codemod-b
          # And @circular/codemod-b uses @circular/codemod-a
```

Instead, restructure your dependencies:

```yaml
# ✅ This is safe and will work correctly
version: "1.0.0"
nodes:
  - id: "foundation"
    steps:
      - name: "Base transformations"
        codemod:
          source: "@safe/base-transform"
          
  - id: "advanced"
    depends_on: ["foundation"]
    steps:
      - name: "Advanced transformations"
        codemod:
          source: "@safe/advanced-transform"
```