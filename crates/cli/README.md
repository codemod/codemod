<p align="center">
  <a href="https://codemod.com">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/codemod/codemod/main/apps/docs/images/intro/codemod-docs-hero-dark.jpg">
      <source media="(prefers-color-scheme: light)" srcset="https://raw.githubusercontent.com/codemod/codemod/main/apps/docs/images/intro/codemod-docs-hero-light.jpg">
      <img alt="Codemod CLI" src="https://raw.githubusercontent.com/codemod/codemod/main/apps/docs/images/intro/codemod-docs-hero-light.jpg">
    </picture>
  </a>

  <p align="center">
    <br />
    <a href="https://go.codemod.com/app">Platform</a>
    ·
    <a href="https://codemod.com/community">Community</a>
    ·
    <a href="https://docs.codemod.com">Docs</a>
  </p>
</p>

# Codemod CLI

[![Community](https://img.shields.io/badge/slack-join-e9a820)](https://codemod.com/community)
[![License](https://img.shields.io/github/license/codemod/codemod)](https://github.com/codemod/codemod/blob/main/LICENSE)
[![npm version](https://img.shields.io/npm/v/codemod.svg)](https://www.npmjs.com/package/codemod)

**Codemod CLI** is an open-source command-line tool for building, testing, and running **codemod packages**—automated code transformations that help teams modernize codebases, upgrade frameworks, and refactor at scale.

Whether you're an individual developer tackling tech debt, an OSS maintainer shipping upgrade paths, or a platform team coordinating migrations across hundreds of services, Codemod CLI gives you the tools to automate repetitive code changes reliably.

## Installation

```bash
npm install -g codemod@latest
```

Or use via `npx` without installation:

```bash
npx codemod@latest <command>
```

## Quick Start

```bash
# 1. Create a codemod package
npx codemod init my-codemod
cd my-codemod

# You can create codemod packages with the help of AI using Codemod MCP or Studio

# 2. Run it locally
npx codemod workflow run -w . -t /path/to/target

# 3. Publish to registry
npx codemod login
npx codemod publish

# 4. Run from registry
npx codemod @your-org/example-codemod
```

## What are Codemod Packages?

**Codemod packages** are portable, reusable code transformation units that can range from simple find-and-replace operations to complex, multi-step migration workflows. Each package includes:

- **Transformation logic** – Written in JavaScript/TypeScript (jssg), YAML ast-grep rules, or shell scripts
- **Workflow definition** – Orchestrates steps, handles dependencies, and manages execution
- **Package manifest** – Defines metadata, target languages, and publishing configuration

Packages are **fully portable**: run them locally during development, in CI/CD pipelines, or share them via the [Codemod Registry](https://go.codemod.com/registry) for your team or the community.

## Why Codemod CLI?

- **🎯 Built for Automation** – Scaffold, test, and publish codemod packages from your terminal
- **📦 Registry Integration** – Share codemods via the [Codemod Registry](https://go.codemod.com/registry) or run community packages instantly
- **⚡ Powerful Engines** – Leverage ast-grep (YAML + jssg) for fast, accurate AST-based transformations
- **🤖 AI-Powered Creation** – Use [Codemod MCP](https://go.codemod.com/mcp-docs) in your IDE or [Codemod Studio](https://go.codemod.com/studio-docs) to build codemods with AI assistance
- **🧪 Built-in Testing** – Validate codemods with snapshot testing before running on production code
- **🔧 Flexible Runtime** – Run directly on your machine or in Docker/Podman containers

## Core Concepts

### Codemod Packages

A **codemod package** is a directory containing:

- `codemod.yaml` – Package metadata (name, version, description, target languages)
- `workflow.yaml` – Workflow steps and orchestration logic
- `scripts/` – JavaScript/TypeScript codemods (jssg)
- `rules/` – YAML ast-grep rule files

Packages can be as simple as a single transformation or as complex as multi-step migration workflows combining JavaScript codemods, YAML rules, shell scripts, and AI-assisted steps.

[Learn more about codemod packages →](https://docs.codemod.com/cli/packages)

### jssg (JavaScript ast-grep)

**jssg** enables you to write codemods in JavaScript/TypeScript that transform code in **any language** supported by ast-grep (JavaScript, TypeScript, Python, Rust, Go, Java, C++, and more).

```typescript
// Example: Replace console.log with logger.info
import type { Transform } from "codemod:ast-grep";
import type TSX from "codemod:ast-grep/langs/tsx";

const transform: Transform<TSX> = (root) => {
  const rootNode = root.root();

  // Find all console.log calls
  const consoleCalls = rootNode.findAll({
    rule: { pattern: "console.log($$$ARGS)" }
  });

  if (consoleCalls.length === 0) {
    return null; // No changes needed
  }

  // Create edits
  const edits = consoleCalls.map((node) => {
    const args = node.getMatch('ARGS')?.text() || '';
    return node.replace(`logger.info(${args})`);
  });

  return rootNode.commitEdits(edits);
};

export default transform;
```

jssg combines the power of AST transformations with the flexibility of JavaScript, making complex transformations intuitive and testable.

[Learn more about jssg →](https://docs.codemod.com/jssg)

### Workflow Orchestration

Workflows define how your codemod package runs. They can orchestrate multiple steps, handle dependencies, manage state, and even include manual approval gates:

```yaml
version: "1"
nodes:
  - id: transform
    name: Update API Calls
    type: automatic
    steps:
      - name: "Run jssg codemod"
        js-ast-grep:
          js_file: "scripts/update-api.ts"
          language: "typescript"
          include:
            - "**/*.ts"
            - "**/*.tsx"
      
      - name: "Format code"
        run: npx prettier --write "**/*.{ts,tsx}"
      
      - name: "Run tests"
        run: npm test
```

[Learn more about workflows →](https://docs.codemod.com/cli/packages/building-workflows)

## CLI Commands

### Package Management

| Command | Description |
|---------|-------------|
| `npx codemod init [path]` | Create a new codemod package with interactive setup |
| `npx codemod publish [path]` | Publish package to the Codemod Registry |
| `npx codemod login` | Authenticate with the registry (browser or API key) |
| `npx codemod logout` | Logout from registry |
| `npx codemod whoami` | Show current authentication status |
| `npx codemod search [query]` | Search for packages in the registry |
| `npx codemod unpublish <package>` | Remove a package from the registry |

### Workflow Commands

| Command | Description |
|---------|-------------|
| `npx codemod workflow run -w <path>` | Run a codemod workflow on your codebase |
| `npx codemod workflow validate -w <path>` | Validate workflow syntax and structure |
| `npx codemod workflow resume -i <id>` | Resume a paused workflow |
| `npx codemod workflow status -i <id>` | Show workflow execution status |
| `npx codemod workflow list` | List recent workflow runs |
| `npx codemod workflow cancel -i <id>` | Cancel a running workflow |

### jssg Commands

| Command | Description |
|---------|-------------|
| `npx codemod jssg run <file> <target> --language <lang>` | Run a jssg codemod directly |
| `npx codemod jssg test <file> --language <lang>` | Test jssg codemod with fixtures |

### Cache Management

| Command | Description |
|---------|-------------|
| `npx codemod cache info` | Show cache statistics |
| `npx codemod cache list` | List all cached packages |
| `npx codemod cache clear [package]` | Clear cache for package or all |
| `npx codemod cache prune` | Remove old or unused cache entries |

**For detailed options and examples, see the [full CLI reference →](https://docs.codemod.com/cli/cli-reference)**

## Ecosystem & Platform

The Codemod CLI is part of a larger ecosystem designed to help teams modernize code at scale:

### Open-Source Tools

- **[Codemod CLI](https://docs.codemod.com/cli)** (this package) – Build, test, and run codemod packages
- **[Codemod MCP](https://go.codemod.com/mcp-docs)** – Build codemods with AI assistance in your IDE
- **[Public Registry](https://go.codemod.com/registry)** – Discover and share community codemods

### Enterprise Platform Features

For teams coordinating migrations across multiple repositories:

- **[Codemod Studio](https://go.codemod.com/studio)** – AI-powered web interface for creating codemods
- **[Campaigns](https://docs.codemod.com/migrations)** – Multi-repo orchestration with progress tracking
- **[Insights](https://docs.codemod.com/insights)** – Analytics dashboards for measuring migration impact
- **Private Registry** – Secure, organization-scoped codemod packages

[Learn more about the platform →](https://app.codemod.com)

## Resources

### Documentation
- **[Full Documentation](https://docs.codemod.com)** – Comprehensive guides and tutorials
- **[CLI Reference](https://docs.codemod.com/cli/cli-reference)** – Detailed command documentation
- **[Codemod Packages](https://docs.codemod.com/cli/packages)** – Learn more about codemod packages and workflows
- **[jssg Documentation](https://docs.codemod.com/jssg)** – JavaScript ast-grep reference

### Get Help
- **[Slack Community](https://codemod.com/community)** – Ask questions and share codemods
- **[GitHub Discussions](https://github.com/codemod/codemod/discussions)** – Long-form Q&A
- **[GitHub Issues](https://github.com/codemod/codemod/issues)** – Report bugs or request features

### Explore
- **[Codemod Registry](https://go.codemod.com/registry)** – Browse community codemods
- **[Codemod Studio](https://go.codemod.com/studio)** – Try creating codemods with AI
- **[Example Codemods](https://github.com/codemod/codemod/tree/main/test-codemods)** – Reference implementations

## Contributing

Contributions are welcome! Help make codemod automation better for everyone.

**Ways to contribute:**
- 🐛 Report bugs via [GitHub Issues](https://github.com/codemod/codemod/issues)
- 💡 Suggest features on [Feedback Board](https://feedback.codemod.com)
- 📝 Improve documentation
- 🔧 Submit pull requests
- 🌟 Star the repo and spread the word

Read our [Contributing Guide](https://github.com/codemod/codemod/blob/main/CONTRIBUTING.md) and [Code of Conduct](https://github.com/codemod/codemod/blob/main/CODE_OF_CONDUCT.md).

## License

MIT License - see [LICENSE](https://github.com/codemod/codemod/blob/main/LICENSE) for details.

---

<p align="center">
  <strong>Built with ❤️ by <a href="https://codemod.com">Codemod</a> and the <a href="https://github.com/codemod/codemod/graphs/contributors">open-source community</a></strong>
</p>
