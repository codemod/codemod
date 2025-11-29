# JSSG Codemod Documentation

This guide provides comprehensive documentation for creating codemods using JavaScript ast-grep (JSSG) on the Codemod platform. JSSG enables powerful, type-safe AST transformations for any language supported by ast-grep.

## Critical Constraints

* **Use only JSSG** on the Codemod platform. Do **not** use jscodeshift, ts-morph, etc.
* **Use the Codemod CLI** for all operations (scaffold, run, test).
* Prefer **TypeScript** everywhere. Use `.ts`/`.tsx` (TSX for JSX) because TS/TSX are supersets of JS and parse JSX more robustly.
* **Never use type `any`** in your code.

## Prerequisites

- Codemod CLI via `npx codemod` (nodejs with npx, pnpx, yarn dlx or bun with bunx)
- Basic understanding of Abstract Syntax Trees (AST)
- TypeScript knowledge
- Familiarity with `codemod-cli-instructions` for project setup and workflow configuration

---

# Part 1: ast-grep Fundamentals

## How ast-grep Works

ast-grep works by:
1. Parsing source code into an AST (Abstract Syntax Tree)
2. Matching nodes in that tree using rules you define
3. Optionally transforming or reporting on matched code

When writing rules, prioritize correctness and clarity over complexity.

## Patterns Are AST Signatures, Not Text

**Core concept:** When you write a `pattern`, ast-grep **parses your snippet into an AST** and matches code with the *same syntactic structure*. It is **not** textual matching.

**Why patterns can fail (JSX example):**
- If you write `pattern: "dangerouslySetInnerHTML={$C}"` without JSX context, the parser treats it as an assignment expression (`dangerouslySetInnerHTML = $C`), not a JSX attribute.
- **Fix:** Either give proper JSX context (`<$E dangerouslySetInnerHTML={$C} $$$REST>`) or use declarative rules with `kind`:

```typescript
// Match JSX attribute by kind instead of pattern
const attrs = rootNode.findAll({
  rule: {
    kind: "jsx_attribute",
    has: {
      kind: "property_identifier",
      regex: "^dangerouslySetInnerHTML$"
    }
  }
});
```

**JSX note:** `jsx_attribute` does NOT expose a field named `"name"`. To match the attribute name, target its `property_identifier` child via `has`.

## Meta-variables: `$`, `$$`, `$$$`

* **`$VAR`**: matches **one** AST node (named nodes by default)
* **`$$VAR`**: matches **one unnamed** node (punctuation, operators, keywords)
* **`$$$VAR`**: matches a **sequence (zero or more)** of nodes (lazy matching)

### CRITICAL: No Partial Matching in Meta-variables

Meta-variables match **entire AST nodes**, not partial text. You **cannot** write `md5$VAR` or `prefix$VAR`.

❌ **WRONG:**
```typescript
// INVALID — you can't do partial matching
rootNode.find({ rule: { pattern: "DigestUtils.md5$VAR($$$ARGS)" } });
```

✅ **CORRECT:**
```typescript
// Capture the whole identifier, then filter with constraints
rootNode.findAll({
  rule: {
    pattern: "DigestUtils.$VAR($$$ARGS)",
    constraints: {
      VAR: { regex: "^md5" }
    }
  }
});
```

## Using `stopBy` for Relational Rules

Relational rules (`has`, `inside`, `precedes`, `follows`) can use `stopBy` to control search depth:

* **`stopBy: "neighbor"`**: search only immediate neighbors
* **`stopBy: "end"`**: search to the boundary (full ascent/descent)
* **`stopBy: <Rule>`**: search until a custom boundary rule matches

```typescript
// Find a string literal anywhere inside a call_expression ancestor
rootNode.findAll({
  rule: {
    kind: "string",
    inside: {
      kind: call_expression",
      stopBy: "end"
    }
  }
});
```

Use `stopBy: "end"` when your relation should be "any ancestor/descendant until the structural boundary."

## Rule Types Reference

**For structural matching**, use `pattern` rules with meta-variables.

**For precise node selection**, combine atomic rules:
- `kind` to match specific AST node types
- `regex` for text-based filtering when structure isn't enough
- `nthChild` for positional selection

**For contextual matching**, leverage relational rules:
- `inside` to ensure nodes appear within specific contexts
- `has` to find nodes containing certain children
- `precedes`/`follows` for sequential relationships

**For complex logic**, use composite rules thoughtfully:
- `all` when multiple conditions must be true
- `any` for alternative patterns
- `not` to exclude specific cases

## YAML Rule Structure (for reference)

While JSSG uses TypeScript, understanding the YAML structure helps:

```yaml
id: descriptive-rule-name
language: JavaScript
rule:
  pattern: 'code pattern with $META_VARS'
constraints:
  META_VAR: { additional rules for the meta-variable }
fix: 'replacement code using $META_VARS'
message: 'Clear explanation of what was found'
```

### Transform Operations

Available transformations for meta-variables in YAML rules:
- `substring`: Extract portions of matched text
- `replace`: Perform text substitution
- `convert`: Change naming conventions (camelCase, snake_case, etc.)

### Pattern Disambiguation

For ambiguous patterns, use context and selector:
```yaml
pattern:
  context: '{ key: value }'
  selector: pair
```

---

# Part 2: Writing JSSG Codemods

## What is JSSG?

JavaScript ast-grep (JSSG) is a TypeScript-based codemod framework that leverages ast-grep's powerful AST pattern matching capabilities while providing:

- **Type Safety**: Full TypeScript support with language-specific AST node types
- **Modular Design**: Organize complex transformations with utility functions
- **Testing Framework**: Built-in test runner with snapshot testing
- **Cross-Platform**: Works seamlessly across different operating systems
- **Language Support**: JavaScript, TypeScript, JSX, TSX, Python, Go, Rust, and more
- **Semantic Analysis**: Find symbol definitions and references across files (JavaScript/TypeScript and Python)

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

## Core API: SgRoot and SgNode

The two fundamental types you'll work with:

```typescript
import type { SgRoot, SgNode } from "@codemod.com/jssg-types/main";
import type TSX from "codemod:ast-grep/langs/tsx";

// SgRoot - represents the entire file
const root: SgRoot<TSX> = /* provided by framework */;
const filename = root.filename(); // Get file path
const rootNode = root.root(); // Get root AST node

// SgNode - represents any AST node
const node: SgNode<TSX> = rootNode.find({
  rule: { pattern: "const $VAR = $VALUE" },
});
```

## Transform Function

Your main transformation logic with proper type annotations:

```typescript
import type { SgRoot, SgNode } from "@codemod.com/jssg-types/main";
import type TSX from "codemod:ast-grep/langs/tsx";

// Main transformation function - return null to skip file
async function transform(root: SgRoot<TSX>): Promise<string | null> {
  const rootNode = root.root();

  // Your transformation logic here
  const edits: Edit[] = [];

  // Collect edits...

  if (edits.length === 0) {
    return null; // No changes needed
  }

  return rootNode.commitEdits(edits);
}

export default transform;
```

## Pattern Matching

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

## Type Guards and Safety

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

## Edit Interface

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

---

# Part 3: Best Practices

## Binding & Context: Don't Assume Names

**Never hard-code names** like `app`, `router`, `jwt`, `useState`. A variable is relevant **because of its origin**, not its spelling.

- Detect bindings (e.g., `const X = express()`), record `X` as the variable of interest, then match `X.get(...)`, etc.
- Always **verify imports** for ambiguous symbols (ensure `useState` is from `'react'`, not a local)
- Support all import shapes: default, namespace, named, CommonJS

❌ **WRONG — Hardcoded for test fixture:**
```typescript
if (methodName === "unsafeEvaluation" && paramName === "userInput") {
  return "sanitizeInput(userInput)";  // Only works for this exact fixture
}
```

✅ **CORRECT — General pattern matching:**
```typescript
// Match ANY code with this structure using meta-variables
const matches = rootNode.findAll({
  rule: { pattern: "$OBJ.evaluate($EXPR)" }
});
// Then verify $OBJ is from a specific package before transforming
```

**Red flags you're doing this wrong:**
- Checking `if (name === "specificName")` for names from your test fixtures
- Logic that only handles exact scenarios in your tests
- Not using meta-variables to capture dynamic parts

## Edits, Inserts, and Commit Strategy

```typescript
async function transform(root: SgRoot<TSX>): Promise<string | null> {
  const rootNode = root.root();
  const edits: Edit[] = []; // Always start with an empty array

  // Collect all edits
  const matches = rootNode.findAll({ rule: { pattern: "console.log($$$ARGS)" } });
  for (const match of matches) {
    edits.push(match.replace("logger.debug($$$ARGS)"));
  }

  // Return null if no changes (not empty string)
  if (edits.length === 0) {
    return null;
  }

  // Commit all edits at once
  return rootNode.commitEdits(edits);
}
```

**Import insertions:** Insert after the last import using `range().end.index`. If no imports exist, insert at file start with separating newlines.

## File Filtering: Use Workflow Globs, Not JavaScript

**Prefer workflow `include`/`exclude` globs** over filtering inside your codemod JavaScript. This is faster and cleaner:

```yaml
# workflow.yaml - PREFERRED approach
steps:
  - js-ast-grep:
      js_file: scripts/codemod.ts
      include:
        - "**/*.ts"
        - "**/*.tsx"
      exclude:
        # `.gitignore`'d files are excluded by default
        - "**/*.test.ts"
        - "**/*.spec.ts"
```

❌ **Avoid filtering in JavaScript:**
```typescript
async function transform(root: SgRoot<TSX>): Promise<string | null> {
  // BAD: Don't filter files in your codemod UNLESS there's not choice for some reason
  if (root.filename().includes("node_modules")) return null;
  if (root.filename().endsWith(".test.ts")) return null;
  // ...
}
```

The workflow engine handles file filtering more efficiently before parsing.

## Handle Edge Cases Gracefully

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

## Optimize Performance

For large codebases, optimize your traversal:

```typescript
// ✅ Good: Single traversal for multiple patterns
const edits: Edit[] = [];
rootNode.findAll({
  rule: {
    any: [
      { pattern: "console.log($$$ARGS)" },
      { pattern: "console.warn($$$ARGS)" },
      { pattern: "console.error($$$ARGS)" },
    ],
  },
}).forEach((node) => {
  // Process all matches in one pass
});

// ❌ Avoid: Multiple separate traversals
rootNode.findAll({ rule: { pattern: "console.log($$$)" } }).forEach(/* ... */);
rootNode.findAll({ rule: { pattern: "console.warn($$$)" } }).forEach(/* ... */);
```

## Prefer `kind` + Declarative Rules Over Brittle Patterns

```typescript
// ✅ Good: Declarative with kind
rootNode.findAll({
  rule: {
    kind: "call_expression",
    has: {
      kind: "member_expression",
      has: { kind: "identifier", regex: "^console$" }
    }
  }
});

// ❌ Brittle: Giant pattern strings
rootNode.findAll({ rule: { pattern: "console.log($A, $B, $C)" } }); // Only matches 3 args
```

## Be Explicit with Transformations

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

---

# Part 4: Semantic Analysis

JSSG provides semantic analysis capabilities for finding symbol definitions and references across files. This is supported for **JavaScript/TypeScript** (using [oxc](https://oxc.rs/)) and **Python** (using [ruff](https://docs.astral.sh/ruff/)). Other languages return no-op results.

## Analysis Modes

- **File Scope**: Single-file analysis (default, fast)
- **Workspace Scope**: Cross-file analysis with import resolution

## Using `node.definition()`

Find where a symbol is defined:

```typescript
async function transform(root: SgRoot<TSX>): Promise<string | null> {
  const rootNode = root.root();

  // Find a symbol reference
  const symbolNode = rootNode.find({
    rule: { pattern: "myFunction" },
  });

  if (!symbolNode) return null;

  // Get its definition
  const def = symbolNode.definition();

  if (def) {
    console.log("Definition in:", def.root.filename());
    console.log("Definition text:", def.node.text());
    console.log("Kind:", def.kind); // 'local', 'import', or 'external'
  }

  return null;
}
```

**Definition kinds:**
- `'local'` — Definition is in the same file
- `'import'` — Traced to an import statement (module couldn't be resolved)
- `'external'` — Definition resolved to a different file in the workspace

### CRITICAL: definition() Returns Different Node Types

**The `def.node` from `definition()` is NOT the same as `getImport().node`**. They return different AST node types:

- For ESM imports (`import x from 'pkg'`), `definition()` returns the `import_clause` node
- For CJS requires (`const x = require('pkg')`), `definition()` returns the `variable_declarator` node

❌ **WRONG — Comparing node IDs directly:**
```typescript
const myImport = getImport(rootNode, { type: "default", from: "my-pkg" });
const def = callee.definition();
// This will NEVER match because they're different node types!
if (def.node.id() === myImport.node.id()) { /* won't work */ }
```

✅ **CORRECT — Verify definition exists and check context**

## Using `node.references()`

Find all references to a symbol across the workspace:

```typescript
async function transform(root: SgRoot<TSX>): Promise<string | null> {
  const rootNode = root.root();
  const currentFile = root.filename();

  // Find a function declaration name
  const funcName = rootNode.find({
    rule: {
      pattern: "formatDate",
      inside: {
        pattern: "export function formatDate($$$PARAMS) { $$$BODY }",
        stopBy: { kind: "export_statement" },
      },
    },
  });

  if (!funcName) return null;

  // Find all references across the workspace
  const refs = funcName.references();
  const currentFileEdits = [];

  for (const fileRef of refs) {
    const edits = fileRef.nodes.map((node) => node.replace("formatDateTime"));

    if (fileRef.root.filename() === currentFile) {
      currentFileEdits.push(...edits);
    } else {
      // Write changes to other files
      const newContent = fileRef.root.root().commitEdits(edits);
      fileRef.root.write(newContent);
    }
  }

  return rootNode.commitEdits(currentFileEdits);
}
```

## Cross-File Editing with `root.write()`

When you have an `SgRoot` from `definition()` or `references()`, you can write changes to other files:

```typescript
// Write to other files (NOT the current file)
const def = node.definition();
if (def && def.root.filename() !== root.filename()) {
  const edits = [def.node.replace("newName")];
  const newContent = def.root.root().commitEdits(edits);
  def.root.write(newContent); // Writes to the other file
}
```

**Important**: You cannot call `write()` on the current file. For the current file, return the modified content from `transform()`.

## Enabling Semantic Analysis in Workflows

Configure semantic analysis in your `workflow.yaml`:

```yaml
version: "1"
nodes:
  transform:
    js-ast-grep:
      js_file: scripts/codemod.ts
      semantic_analysis: workspace  # or "file" for single-file mode
```

Run with:

```bash
npx codemod workflow run -w /path/to/workflow.yaml -t /path/to/target
```

---

# Part 5: Testing (CRITICAL)

## Test Structure — Use This Exact Layout

Each test case is a folder with an `input`/`expected` pair:

```
tests/
├── positive-case-1/
│   ├── input.ts       # Code before transformation
│   └── expected.ts    # Expected output
├── positive-case-2/
│   ├── input.tsx
│   └── expected.tsx
├── negative-case-1/   # Code that should NOT change
│   ├── input.ts
│   └── expected.ts    # Same as input
└── edge-case-1/
    ├── input.ts
    └── expected.ts
```

**Guidelines:**
- **Positive cases**: All specified transformations happen
- **Negative cases**: Code remains unchanged
- **Edge cases**: Import variants, nested forms, complex expressions, comments preservation
- **Prefer `.ts`/`.tsx`** even when source is `.js`/`.jsx`
- If a test fails only due to formatting but AST is correct, **update the expected output**

## Running Tests

```bash
# Run all tests (use pnpm test which calls jssg test)
pnpm test

# Update expected outputs when changes are intended
pnpm test -u

# Run specific test
pnpm test --filter <test-name>

# Verbose output for debugging
npx codemod jssg test -l tsx ./scripts/codemod.ts -v
```

## Writing Effective Tests

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

---

# Part 6: API Reference

## SgNode Methods

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
  
  // Semantic Analysis Methods (JavaScript/TypeScript and Python only)
  /** Find the definition of the symbol at this node's position */
  definition(options?: { resolveExternal?: boolean }): DefinitionResult | null;
  /** Find all references to the symbol at this node's position */
  references(): Array<FileReferences>;
}
```

## SgRoot Methods

```typescript
class SgRoot<M> {
  root(): SgNode<M>;
  filename(): string;
  /** Write content to this file (only for files from definition()/references()) */
  write(content: string): void;
}
```

## Semantic Types

```typescript
interface DefinitionResult<M> {
  node: SgNode<M>;
  root: SgRoot<M>;
  kind: 'local' | 'import' | 'external';
}

interface FileReferences<M> {
  root: SgRoot<M>;
  nodes: Array<SgNode<M>>;
}
```

---

# Part 7: Troubleshooting

## Common Issues and Solutions

### 1. Transformation Not Applied

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

### 2. Type Errors

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

### 3. Test Failures

**Issue**: Tests fail unexpectedly.

**Debug with verbose output**:

```bash
npx codemod jssg test -l tsx ./scripts/codemod.ts -v
```

**Update snapshots if changes are intended**:

```bash
npx codemod jssg test -l tsx ./scripts/codemod.ts -u
```

### 4. Pattern Matching Issues

**Issue**: Patterns don't match expected code.

**Use MCP tools to debug patterns:**

1. Use `dump_ast` tool with your code sample to see the AST structure
2. Use `get_node_types` tool to see available node kinds for the language
3. Compare the AST output with your pattern to understand mismatches

## Performance Optimization

For large codebases:

1. **Early returns**: Skip files that don't need transformation
2. **Efficient patterns**: Use specific patterns over generic ones
3. **Batch operations**: Collect all edits before committing
4. **Limit traversals**: Combine related searches

---

# Quality Bar & Anti-Pitfalls

* **Package correctness**: Transformations must only apply to the intended library API — verify imports/bindings first
* **No string-hacks**: Operate on AST nodes; use text checks only when intentionally heuristic
* **Never write fixture-specific code**: Your codemod must be general and work with ANY code matching the pattern
* **Debug properly**: Use the `dump_ast` MCP tool, log `node.kind()` and `node.text()` to investigate mismatches
* **Performance**: Use specific `kind` filters; combine related checks with `any`/`all`; early-exit aggressively

---

# Part 8: Common AST Patterns

## NEVER Use String Operations When AST Can Be Used

String operations like `.includes()`, `.startsWith()`, `.match()` on node text are **anti-patterns** when you're checking AST structure. Always use AST queries instead.

### Checking Object Properties

❌ **WRONG — String operations:**
```typescript
const optionsText = optionsArg.text();
const hasStyles = optionsText.includes("styles:");
const hasLabel = optionsText.includes("label:");
```

✅ **CORRECT — AST query:**
```typescript
function hasPropertyInObject(objectNode: SgNode<Language>, propertyName: string): boolean {
  const pairs = objectNode.findAll({
    rule: {
      kind: "pair",
      has: {
        kind: "property_identifier",
        regex: `^${propertyName}$`,
      },
    },
  });
  return pairs.length > 0;
}

const hasStyles = hasPropertyInObject(optionsArg, "styles");
const hasLabel = hasPropertyInObject(optionsArg, "label");
```

### Getting String Content Without Quotes

❌ **WRONG — String slicing:**
```typescript
const pathText = pathArg.text();
const pathContent = pathText.slice(1, -1); // Remove quotes - fragile!
```

✅ **CORRECT — AST child node:**
```typescript
function getStringContent(node: SgNode<Language>): string | null {
  if (!node.is("string")) return null;
  
  // Find the string_fragment child which contains the actual content
  const fragment = node.find({
    rule: { kind: "string_fragment" },
  });
  if (fragment) {
    return fragment.text();
  }
  return null;
}
```

### Finding Comments Before Statements

Comments before a statement are **siblings**, not children. You have two approaches:

❌ **WRONG — Looking in children:**
```typescript
const comments = callNode.findAll({ rule: { kind: "comment" } });
// Won't find comments on the line before the call!
```

✅ **CORRECT Using `follows()` with regex:**
```typescript
function hasIgnoreComment(callNode: SgNode<Language>): boolean {
  // Get the expression statement containing this call
  const exprStmt = callNode.ancestors().find((a) => a.kind() === "expression_statement");
  if (!exprStmt) return false;

  // Use follows() to check if this statement follows a specific comment
  return exprStmt.follows({
    rule: {
      kind: "comment",
      regex: "//\s*codemod-ignore",
    },
  });
}
```

**When to use which:**
- Use `follows()` when you just need to check if a comment with a pattern exists before the node
- Use `prevAll()` when you need more control (e.g., only check the immediately preceding comment, or extract values from the comment)

### Finding Something in Arguments

❌ **WRONG — String search:**
```typescript
const argsText = argsNode.text().toLowerCase();
if (argsText.includes("ratelimit") || argsText.includes("limiter")) {
  // ...
}
```

✅ **CORRECT — Using `has()` with regex:**
```typescript
function hasRateLimitingInArgs(argsNode: SgNode<Language>): boolean {
  // Use has() to check if args contain an identifier matching rate limiting patterns
  // (?i) makes it case-insensitive
  return argsNode.has({
    rule: {
      kind: "identifier",
      regex: "(?i)(ratelimit|limiter|throttle)",
    },
  });
}
```

### Checking for Specific Call Patterns

❌ **WRONG — String contains:**
```typescript
if (stmt.text().includes("require(") || stmt.text().includes("express()")) {
  // ...
}
```

✅ **CORRECT — AST pattern matching:**
```typescript
const hasRequire = stmt.find({
  rule: {
    kind: "call_expression",
    has: {
      field: "function",
      kind: "identifier",
      regex: "^require$",
    },
  },
});

const hasExpressCall = stmt.find({
  rule: {
    kind: "call_expression",
    has: {
      field: "function",
      kind: "identifier",
    },
  },
});
```

## When String Checks ARE Acceptable

String checks are acceptable when:
1. **Checking actual string VALUES** (not AST structure): e.g., checking if a string literal contains `process.env`
2. **Comment content**: Comments are text by nature
3. **Regex patterns**: When the pattern itself needs text matching
4. **Quote style preservation**: e.g., `value.startsWith('"')` to maintain quote consistency

---

# CLI Commands Reference

## Initialize a Project

```bash
npx codemod@latest init [OPTIONS] [PATH]

Options:
  --name               Project name
  --project-type       Project type (ast-grep-js, hybrid, shell, ast-grep-yaml)
  --package-manager    Package manager (npm, yarn, pnpm)
  --language          Target language
  --no-interactive   Use defaults without prompts
```

## JSSG Commands

```bash
# Execute codemod on target files/directories
npx codemod jssg run [OPTIONS] <CODEMOD_FILE> <TARGET>

# Test codemod
npx codemod jssg test [OPTIONS] <CODEMOD_FILE> [TEST_DIR]

Options:
  -l, --language         Language to process (required)
  --filter              Run only tests matching pattern
  -u, --update-snapshots Update expected outputs
  -v, --verbose         Show detailed output
  --fail-fast          Stop on first failure
```

## Publishing

```bash
npx codemod@latest login
npx codemod@latest publish
npx codemod@latest search <query>
npx codemod@latest run <package-name>
```
