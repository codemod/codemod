# JavaScript ast-grep (JSSG) Codemod Documentation

This guide provides comprehensive documentation for creating and working with JavaScript ast-grep (JSSG) codemods using the Codemod CLI. JSSG enables powerful, type-safe AST transformations for any codebases of any language supported by ast-grep.

JSSG provides a powerful, type-safe approach to JavaScript and TypeScript code transformation. By following these guidelines and best practices, you can create robust codemods that:

- Transform code reliably and predictably
- Handle edge cases gracefully
- Maintain high performance on large codebases
- Provide excellent developer experience

## Prerequisites
You MUST check `ast-grep-instructions` and `codemod-cli-instructions` instructions in addition to this doc. Do not continue without checking those docs.

## Introduction

### What is JSSG?

JavaScript ast-grep (JSSG) is a TypeScript-based codemod framework that leverages ast-grep's powerful AST pattern matching capabilities while providing:

- **Type Safety**: Full TypeScript support with language-specific AST node types
- **Modular Design**: Organize complex transformations with utility functions
- **Testing Framework**: Built-in test runner with snapshot testing
- **Cross-Platform**: Works seamlessly across different operating systems
- **Language Support**: JavaScript, TypeScript, JSX, TSX, Python, Go, Rust, and more

### When to Use JSSG

JSSG excels at structural code transformations that require:

- Complex AST traversal and manipulation
- Type-safe node operations
- Reusable transformation logic
- Comprehensive test coverage

## Getting Started

### Prerequisites

- Codemod CLI via `npx codemod` (nodejs with npx, pnpx, yarn dlx or bun with bunx)
- Basic understanding of Abstract Syntax Trees (AST)
- TypeScript knowledge

### Creating a New JSSG Project

Initialize a new JSSG codemod project with explicit configuration:

```bash
npx codemod@latest init my-codemod \
  --project-type ast-grep-js \
  --language typescript \
  --package-manager pnpm
  --no-interactive
```

### Project Types Available

- `ast-grep-js`: JavaScript/TypeScript ast-grep codemod (recommended)
- `hybrid`: Multi-step workflow combining Shell, YAML, and JSSG
- `shell`: Shell command workflow (legacy)
- `ast-grep-yaml`: YAML-based ast-grep codemod (legacy)

## Project Structure

A typical JSSG codemod project follows this structure:

```
my-codemod/
├── codemod.yaml          # Codemod metadata and configuration
├── package.json          # Node.js dependencies
├── tsconfig.json         # TypeScript configuration
├── scripts/
│   ├── codemod.ts       # Main transformation logic
│   └── utils/           # Reusable utility functions
│       ├── ast-utils.ts
│       └── helpers.ts
└── tests/               # Test fixtures
    └── test-case-1/
        ├── input.tsx    # Code before transformation
        └── expected.tsx # Expected output
```

### Essential Files

#### codemod.yaml and workflow.yaml

You must check codemod cli instructions to understand how to initialize the project.

#### scripts/codemod.ts

Your main transformation logic with proper type annotations:

```typescript
import type { SgRoot, SgNode } from "@codemod.com/jssg-types/main";
import type TSX from "codemod:ast-grep/langs/tsx";

// Main transformation function - return null to skip file
async function transform(root: SgRoot<TSX>): Promise<string | null> {
  const rootNode = root.root();

  // Your transformation logic here
  // Use explicit checks and type guards

  const edits = collectEdits(rootNode); // Must be an array of Edit

  if (edits.length === 0) {
    return null; // No changes needed
  }

  return rootNode.commitEdits(edits);
}

export default transform;
```

## Writing JSSG Codemods

### Core API Concepts

#### SgRoot and SgNode

The two fundamental types you'll work with:

```typescript
// SgRoot - represents the entire file
const root: SgRoot<TSX> = /* provided by framework */;
const filename = root.filename(); // Get file path
const rootNode = root.root(); // Get root AST node

// SgNode - represents any AST node
const node: SgNode<TSX> = rootNode.find({
  rule: { pattern: "const $VAR = $VALUE" },
});
```

#### Pattern Matching

Use ast-grep patterns to find specific code structures:

```typescript
// Find all console.log calls
const consoleLogs = rootNode.findAll({
  rule: { pattern: "console.log($$$ARGS)" }
});

// Find with multiple patterns
const imports = rootNode.findAll({
  rule: {
    any: [
      { pattern: "import $NAME from $SOURCE" },
      { pattern: "import { $$$NAMES } from $SOURCE" },
      { pattern: "import * as $NAME from $SOURCE" },
    ],
  },
});
```

#### Type Guards and Safety

Always verify node types before operations:

```typescript
// Type-safe node checking
if (node.is("arrow_function") || node.is("function_declaration")) {
  const params = node.field("parameters");
  // Safe to access function-specific fields
}

// Check for required fields
const typeAnnotation = node.field("type")?.child(1);
if (!typeAnnotation) {
  return null; // Skip if no type annotation
}
```

#### Edit

```typescript
interface Edit {
  /** The start position of the edit */
  startPos: number;
  /** The end position of the edit */
  endPos: number;
  /** The text to be inserted */
  insertedText: string;
}

const edit1 = {
  startPos: node.range().start.index,
  endPos: typeAnnotation.range().start.index,
  insertedText: "// TODO: comment to add before node\n",
};

// Or simply replace a node
const edit2 = anotherNode.replace("logger.log()");

rootNode.commitEdits([edit1, edit2]);
```

### Practical Example: Next.js Route Props Transform

Here's a complete example that adds type annotations to Next.js page components:

```typescript
import type { SgRoot, SgNode } from "@codemod.com/jssg-types/main";
import type TSX from "codemod:ast-grep/langs/tsx";
import { getDefaultExport, isFunctionLike } from "./utils/ast-utils";
import { getNextResolvedRoute } from "./utils/next-routes";

async function transform(root: SgRoot<TSX>): Promise<string | null> {
  const rootNode = root.root();

  // Step 1: Find the default export
  const defaultExport = getDefaultExport(rootNode);
  if (!defaultExport || !isFunctionLike(defaultExport)) {
    return null; // Not a function component
  }

  // Step 2: Check for parameters that need typing
  const secondParam = defaultExport.field("parameters")?.child(1);

  if (!secondParam?.is("required_parameter")) {
    return null; // No second parameter or already destructured
  }

  // Step 3: Get the parameter's type annotation
  const typeAnnotation = secondParam.field("type")?.child(1);
  if (!typeAnnotation) {
    return null; // Already typed
  }

  // Step 4: Determine the route and component type
  const filePath = root.filename();
  const route = await getNextResolvedRoute(filePath);

  if (!route) {
    return null; // Not in a Next.js app directory
  }

  const typeName = filePath.endsWith("/layout.tsx")
    ? "LayoutProps"
    : "PageProps";

  // Step 5: Apply the transformation
  const edit = typeAnnotation.replace(`${typeName}<"${route}">`);
  return rootNode.commitEdits([edit]);
}

export default transform;
```

### Creating Utility Functions

Organize reusable logic in utility modules:

```typescript
// utils/ast-utils.ts
import type { SgNode } from "@codemod.com/jssg-types/main";
import type TSX from "codemod:ast-grep/langs/tsx";

export function getDefaultExport<T extends TSX>(
  program: SgNode<T, "program">
): SgNode<T> | null {
  const exportDefault = program.find({
    rule: { pattern: "export default $ARG" },
  });

  const arg = exportDefault?.getMatch("ARG");
  if (!arg) return null;

  // Follow identifier references
  if (arg.is("identifier")) {
    return findDefinition(arg);
  }

  return arg;
}

export function isFunctionLike<T extends TSX>(
  node: SgNode<T>
): node is SgNode<T, "function_declaration" | "arrow_function"> {
  return (
    node.is("function_declaration") ||
    node.is("arrow_function") ||
    node.is("function_expression")
  );
}
```

## Testing Your Codemod

### Test Structure

JSSG uses snapshot testing with input/expected file pairs:

```
tests/
├── basic-transform/
│   ├── input.ts       # Original code
│   └── expected.ts    # Expected output
├── edge-cases/
│   ├── input.tsx
│   └── expected.tsx
└── complex-scenario/
    ├── nested-dir/
    │   ├── input.js
    │   └── expected.js
    └── config.json    # Optional test configuration
```

### Running Tests

Execute all tests:

```bash
npx codemod jssg test -l tsx ./scripts/codemod.ts
```

Run specific test directories:

```bash
npx codemod jssg test -l tsx ./scripts/codemod.ts tests/basic-transform
```

Advanced test options:

```bash
# Update snapshots when output changes are intended
npx codemod jssg test -l tsx ./scripts/codemod.ts -u

# Run tests matching a pattern
npx codemod jssg test -l tsx ./scripts/codemod.ts --filter "edge-cases"

# Verbose output for debugging
npx codemod jssg test -l tsx ./scripts/codemod.ts -v

# Run tests sequentially (helpful for debugging)
npx codemod jssg test -l tsx ./scripts/codemod.ts --sequential
```

### Writing Effective Tests

Create comprehensive test cases that cover:

1. **Basic transformations**: Simple, happy-path scenarios
2. **Edge cases**: Boundary conditions and unusual inputs
3. **No-op cases**: Code that shouldn't be transformed
4. **Complex scenarios**: Real-world code patterns

Example test case:

```typescript
// tests/react-hooks/input.tsx
import React from "react";

// Should transform: missing dependency
function Component() {
  const [count, setCount] = useState(0);

  useEffect(() => {
    console.log(count);
  }, []); // Missing 'count' dependency

  return <div>{count}</div>;
}

// Should NOT transform: correct dependencies
function CorrectComponent() {
  const [value] = useState(0);

  useEffect(() => {
    console.log(value);
  }, [value]); // Correct dependency

  return <div>{value}</div>;
}
```

## CLI Commands Reference

### Core Commands

#### Initialize a Project

```bash
npx codemod@latest init [OPTIONS] [PATH]

Options:
  --name               Project name
  --project-type       Project type (ast-grep-js, hybrid, shell, ast-grep-yaml)
  --package-manager    Package manager (npm, yarn, pnpm)
  --language          Target language
  --description       Project description
  --author           Author name and email
  --license          License type
  --private          Make package private
  --force            Overwrite existing files
  --no-interactive   Use defaults without prompts
```

#### JSSG-Specific Commands

##### Bundle

```bash
npx codemod jssg bundle <FILE>
# Bundles TypeScript files and dependencies into a single file
```

##### Run

```bash
npx codemod jssg run [OPTIONS] <CODEMOD_FILE> <TARGET>
# Execute codemod on target files/directories
```

##### Test

```bash
npx codemod jssg test [OPTIONS] <CODEMOD_FILE> [TEST_DIR]

Options:
  -l, --language         Language to process (required)
  --filter              Run only tests matching pattern
  -u, --update-snapshots Update expected outputs
  -v, --verbose         Show detailed output
  --sequential          Run tests sequentially
  --max-threads         Maximum concurrent threads
  --fail-fast          Stop on first failure
  --watch              Watch for changes
  --timeout            Test timeout in seconds
```

### Publishing and Distribution

```bash
# Login to registry
npx codemod@latest login

# Publish your codemod
npx codemod@latest publish

# Search for codemods
npx codemod@latest search <query>

# Run published codemod
npx codemod@latest run <package-name>
```

## Best Practices

### 1. Be Explicit with Transformations

JSSG codemods should be precise and predictable. Always include explicit checks:

```typescript
// ✅ Good: Explicit validation
const param = node.field("parameters")?.child(0);
if (!param || !param.is("required_parameter")) {
  return null;
}

// ❌ Avoid: Assumptions without checks
const param = node.field("parameters").child(0); // May throw
```

### 2. Provide Context in Your Code

Add clear comments explaining complex transformations:

```typescript
async function transform(root: SgRoot<TSX>): Promise<string | null> {
  const rootNode = root.root();

  // Find all useEffect hooks that might have missing dependencies
  // This helps prevent React Hook exhaustive-deps violations
  const effects = rootNode.findAll({
    rule: { pattern: "useEffect($CALLBACK, $DEPS)" }
  });

  // Process each effect to ensure correct dependency array
  const edits = effects.flatMap((effect) => {
    // Detailed logic with explanations...
  });
}
```

### 3. Handle Edge Cases Gracefully

Your codemod should handle various code styles and edge cases:

```typescript
// Handle different import styles
const imports = rootNode.findAll({
  rule: {
    any: [
      { pattern: "import $NAME from $SOURCE" }, // Default import
      { pattern: "import { $$$NAMES } from $SOURCE" }, // Named imports
      { pattern: "import * as $NAME from $SOURCE" }, // Namespace import
      { pattern: "const $NAME = require($SOURCE)" }, // CommonJS
    ],
  },
});
```

### 4. Optimize Performance

For large codebases, optimize your traversal:

```typescript
// ✅ Good: Single traversal collecting all edits
const edits: Edit[] = [];
rootNode.findAll({ rule: { pattern: "console.log($$$)" } }).forEach((node) => {
  edits.push(node.replace("logger.debug($$$)"));
});
return rootNode.commitEdits(edits);

// ❌ Avoid: Multiple traversals
rootNode.findAll({ rule: { pattern: "console.log($$$)" } }).forEach(/* ... */);
rootNode.findAll({ rule: { pattern: "console.warn($$$)" } }).forEach(/* ... */);
```

### 5. Write Comprehensive Tests

Include tests for all transformation scenarios:

```typescript
// tests/all-scenarios/input.ts
// Test 1: Basic transformation
const simple = "before";

// Test 2: Should not transform
const preserved = "unchanged";

// Test 3: Edge case with comments
const withComment = /* comment */ "before";

// Test 4: Complex nested structure
const nested = {
  value: "before",
  inner: {
    deep: "before",
  },
};
```

### 6. Use Type-Safe Patterns

Leverage TypeScript's type system:

```typescript
import type { SgNode } from "@codemod.com/jssg-types/main";
import type TSX from "codemod:ast-grep/langs/tsx";

// Type-safe node type checking
function isReactComponent(
  node: SgNode<TSX>
): node is SgNode<TSX, "function_declaration" | "arrow_function"> {
  if (!isFunctionLike(node)) return false;

  // Check if it returns JSX
  const returnStatements = node.findAll({
    rule: { pattern: "return $EXPR" },
  });

  return returnStatements.some((ret) => {
    const expr = ret.getMatch("EXPR");
    return expr?.kind() === "jsx_element" || expr?.kind() === "jsx_fragment";
  });
}
```

## Advanced Patterns

### Async Transformations

JSSG supports async operations for complex transformations:

```typescript
async function transform(root: SgRoot<TSX>): Promise<string | null> {
  const filePath = root.filename();

  // Async file system operations
  const config = await loadProjectConfig(filePath);

  // Async API calls or analysis
  const metadata = await analyzeImports(root.root());

  // Use gathered data in transformation
  const edits = await generateEdits(root.root(), config, metadata);

  return root.root().commitEdits(edits);
}
```

### Multi-File Context

Access information from other files:

```typescript
import { findProjectRoot, analyzeExports } from "./utils/project-analysis";

async function transform(root: SgRoot<TSX>): Promise<string | null> {
  const projectRoot = await findProjectRoot(root.filename());

  // Analyze exports from related files
  const componentExports = await analyzeExports(projectRoot, "src/components");

  // Use cross-file information in transformation
  const edits = transformImports(root.root(), componentExports);

  return root.root().commitEdits(edits);
}
```

### Custom Node Matchers

Create reusable, complex matchers:

```typescript
// utils/matchers.ts
export function createReactHookMatcher() {
  return {
    rule: {
      pattern: "$HOOK($$$ARGS)",
      where: {
        HOOK: {
          regex: "^use[A-Z]", // Matches useEffect, useState, etc.
        },
      },
    },
  };
}

// Usage in codemod
const hooks = rootNode.findAll(createReactHookMatcher());
```

## Troubleshooting

### Common Issues and Solutions

#### 1. Transformation Not Applied

**Issue**: Your codemod runs but doesn't make changes.

**Debug steps**:

```typescript
async function transform(root: SgRoot<TSX>): Promise<string | null> {
  const rootNode = root.root();

  // Add debug logging
  console.log("Processing file:", root.filename());

  const matches = rootNode.findAll({ rule: { pattern: "your pattern" } });
  console.log("Found matches:", matches.length);

  if (matches.length === 0) {
    console.log("No matches found - check your pattern");
    return null;
  }

  // Log each match for inspection
  matches.forEach((match, i) => {
    console.log(`Match ${i}:`, match.text());
  });
}
```

#### 2. Type Errors

**Issue**: TypeScript compilation errors.

**Solution**: Ensure proper type imports and annotations:

```typescript
// Always import these types
import type { SgRoot, SgNode, Edit } from "@codemod.com/jssg-types/main";
import type TSX from "codemod:ast-grep/langs/tsx";

// Use proper type annotations
function helper(node: SgNode<TSX>): Edit | null {
  // Implementation
}
```

#### 3. Test Failures

**Issue**: Tests fail unexpectedly.

**Debug with verbose output**:

```bash
npx codemod jssg test -l tsx ./scripts/codemod.ts -v
```

**Update snapshots if changes are intended**:

```bash
npx codemod jssg test -l tsx ./scripts/codemod.ts -u
```

#### 4. Pattern Matching Issues

**Issue**: Patterns don't match expected code.

**Use the ast-grep playground** to test patterns:

1. Visit https://ast-grep.github.io/playground/
2. Paste your code sample
3. Test different patterns
4. Use the AST viewer to understand structure

### Performance Optimization

For large codebases:

1. **Early returns**: Skip files that don't need transformation
2. **Efficient patterns**: Use specific patterns over generic ones
3. **Batch operations**: Collect all edits before committing
4. **Limit traversals**: Combine related searches

```typescript
async function transform(root: SgRoot<TSX>): Promise<string | null> {
  const rootNode = root.root();

  // Early return for non-applicable files
  if (!root.filename().endsWith(".tsx")) {
    return null;
  }

  // Single traversal for multiple patterns
  const edits: Edit[] = [];

  rootNode
    .findAll({
      rule: {
        any: [
          { pattern: "console.log($$$ARGS)" },
          { pattern: "console.warn($$$ARGS)" },
          { pattern: "console.error($$$ARGS)" },
        ],
      },
    })
    .forEach((node) => {
      const callee = callExpressionNode.field("function");
      const method = callee.field("property")?.text();
      const args = callExpressionNode
        .getMultipleMatches("ARG")
        .map(x => x.isNamed() ? x.text() : null)
        .filter(x => x !== null)
        .join(", ");
      edits.push(callExpressionNode.replace(`logger.${method}(${args})`));
    });

  return edits.length > 0 ? rootNode.commitEdits(edits) : null;
}
```

### SgNode methods

```typescript
class SgNode<
  M extends TypesMap = TypesMap,
  out T extends Kinds<M> = Kinds<M>
> {
  /** Returns the node's id */
  id(): number;
  range(): Range;
  isLeaf(): boolean;
  isNamed(): boolean;
  isNamedLeaf(): boolean;
  text(): string;
  matches(m: string | number | RuleConfig<M>): boolean;
  inside(m: string | number | RuleConfig<M>): boolean;
  has(m: string | number | RuleConfig<M>): boolean;
  precedes(m: string | number | RuleConfig<M>): boolean;
  follows(m: string | number | RuleConfig<M>): boolean;
  /** Returns the string name of the node kind */
  kind(): T;
  readonly kindToRefine: T;
  /** Check if the node is the same kind as the given `kind` string */
  is<K extends T>(kind: K): this is SgNode<M, K>;
  // we need this override to allow string literal union
  is(kind: string): boolean;

  getMatch: NodeMethod<M, [mv: string]>;
  getMultipleMatches(m: string): Array<SgNode<M>>;
  getTransformed(m: string): string | null;
  /** Returns the node's SgRoot */
  getRoot(): SgRoot<M>;
  children(): Array<SgNode<M>>;
  find: NodeMethod<M, [matcher: string | number | RuleConfig<M>]>;
  findAll<K extends Kinds<M>>(
    matcher: string | number | RuleConfig<M>
  ): Array<RefineNode<M, K>>;
  /** Finds the first child node in the `field` */
  field<F extends FieldNames<M[T]>>(name: F): FieldNode<M, T, F>;
  /** Finds all the children nodes in the `field` */
  fieldChildren<F extends FieldNames<M[T]>>(
    name: F
  ): Exclude<FieldNode<M, T, F>, null>[];
  parent: NodeMethod<M>;
  child(nth: number): SgNode<M, ChildKinds<M, T>> | null;
  child<K extends NamedChildKinds<M, T>>(nth: number): RefineNode<M, K> | null;
  ancestors(): Array<SgNode<M>>;
  next: NodeMethod<M>;
  nextAll(): Array<SgNode<M>>;
  prev: NodeMethod<M>;
  prevAll(): Array<SgNode<M>>;
  replace(text: string): Edit;
  commitEdits(edits: Array<Edit>): string;
}
```

When building JSSG codemods, always test your work using the jssg test command or the mcp tool for testing.