# Contributing to Codemod

Thank you for your interest in contributing to Codemod! This guide will help you get started.

This repository contains **Codemod's open-source tooling**: the CLI and core libraries that power code migrations, framework upgrades, and large-scale changes. We welcome contributions from the community to help make code modernization accessible to everyone.

> **Note:** This repository focuses on the open-source CLI and core libraries. Codemod platform (web app) is part of Codemod's enterprise features and is maintained by the Codemod core team.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Ways to Contribute](#ways-to-contribute)
- [Getting Started](#getting-started)
- [Development Setup](#development-setup)
- [Project Structure](#project-structure)
- [Making Changes](#making-changes)
- [Testing](#testing)
- [Submitting Changes](#submitting-changes)
- [Getting Help](#getting-help)

## Code of Conduct

This project follows our [Code of Conduct](./CODE_OF_CONDUCT.md). By participating, you are expected to uphold this code.

## Ways to Contribute

We welcome contributions in several areas:

### 📚 Documentation

Help improve our documentation (`docs/`) to make Codemod more accessible:

- **Documentation improvements** – Fix typos & inaccuracies, clarify explanations, add examples
- **Tutorials and guides** – Share your knowledge with the community

### 🌐 Website

Found a bug on [codemod.com](https://codemod.com)? We'd love your help fixing it (`apps/frontend/`).

### 🛠️ Tooling

> We especially welcome bug fixes and moderate improvements to existing tooling. Drastic changes to user experience or major feature additions typically require broader alignment, as they affect many users and workflows—these are often discussed with the core team internally first.  
>
> If you have a cool new idea or want to explore bigger changes, please **ping us in the [community channel](https://codemod.com/community)** before submitting a PR. We’d love to chat and help shape the direction together!

Help improve the core tools that power Codemod:

#### CLI (`crates/cli/`)

The Rust-based CLI is the heart of Codemod. Contributions welcome for:

- New features and commands
- Performance improvements
- Bug fixes
- Better error messages and user experience


#### jssg & workflows (`crates/core/`, `crates/codemod-sandbox/`)

The workflow engine and JavaScript ast-grep execution:

- **Workflow engine** (`crates/core/`) – Core execution engine improvements
- **jssg execution** (`crates/codemod-sandbox/`) – JavaScript/TypeScript codemod execution
- **Workflow features** – New step types, better error handling, performance optimizations

#### Codemod MCP (`crates/mcp/`)

The Model Context Protocol server for AI-powered codemod creation:

- **MCP tools** – New analysis and transformation tools
- **AI improvements** – Improvements to AI-powered codemod generation
- **IDE support** – Better integration with AI-powered IDEs

### 📦 Registry

The best way to contribute to the registry is by **publishing codemods**:

- **Publish codemods** – Share your transformations with the community
- **Improve existing codemods** – Submit improvements to community codemods
- **Document codemods** – Help others discover and use codemods effectively

Learn more about publishing codemods in the [Registry documentation](https://docs.codemod.com/registry).

[Explore Codemod Registry ->](https://codemod.com/registry)

## Getting Started

### Prerequisites

- **Node.js** >= 20.x
- **pnpm** >= 8.x
- **Rust** (latest stable) – for CLI and core development

## Development Setup

### Run Documentation Local Server

```bash
# Install Mintlify
npm i -g mint

# Run documentation site (in docs/)
mint dev
```

### Install Dependencies

```bash
# Install Node.js dependencies
pnpm install

# Rust dependencies are automatically managed by Cargo
# No separate installation needed
```

### Build the Project

```bash
# Build all packages (TypeScript/JavaScript)
pnpm build

# Build the Rust CLI
cargo build --release

# Or build from the CLI directory
cd crates/cli
cargo build --release
```

### Run Development Servers

```bash
# Run frontend (marketing website)
pnpm --filter @codemod-com/frontend dev
```

### Test the CLI

```bash
# Build the CLI
cargo build --release --package codemod --bin codemod

# Test CLI commands
./target/release/codemod --help
./target/release/codemod workflow --help
./target/release/codemod jssg --help
```

## Project Structure

This monorepo contains the open-source Codemod CLI and core libraries:

### Core Components

- **`crates/cli/`** – Main Rust-based CLI (the active CLI)
- **`crates/core/`** – Workflow engine (butterflow-core)
- **`crates/codemod-sandbox/`** – JavaScript/TypeScript execution sandbox for jssg
- **`crates/mcp/`** – Model Context Protocol server for AI-powered codemod creation
- **`crates/telemetry/`** – Telemetry and analytics
- **`crates/ai/`** – AI tools and integrations

### Shared Packages

- **`packages/jssg-types/`** – TypeScript types for jssg codemods
- **`packages/tsconfig/`** – Shared TypeScript configuration
- **`packages/codemod-utils/`** – Public utilities for codemod authors

### Applications

- **`docs/`** – Documentation website (Mintlify)
- **`apps/frontend/`** – Marketing website (Next.js)

> **Note:** Legacy components have been moved to a `legacy` branch.

## Making Changes

### Development Workflow

1. **Make your changes** in the appropriate directory
2. **Write or update tests** for your changes
3. **Run tests** to ensure everything passes
4. **Lint your code**:
   ```bash
   # TypeScript/JavaScript
   pnpm lint
   
   # Rust
   cargo fmt --check
   cargo clippy
   ```
5. **Format your code**:
   ```bash
   # TypeScript/JavaScript
   pnpm lint:write
   
   # Rust
   cargo fmt
   ```

### Code Style

- **TypeScript/JavaScript**: We use [Biome](https://biomejs.dev/) for linting and formatting
- **Rust**: We use `rustfmt` (configured in `rustfmt.toml`) and `clippy`
- **Documentation**: Follow existing patterns and use clear, concise language
- Write clear, self-documenting code
- Add comments for complex logic

## Testing

### Run Tests

```bash
# Run all TypeScript/JavaScript tests
pnpm test

# Run Rust tests
cargo test
```

### Writing Tests

- Write tests for new features and bug fixes
- Aim for good test coverage
- Use descriptive test names
- Test both happy paths and edge cases
- For CLI changes, test the actual command execution

### Testing CLI Changes

```bash
# Build the CLI
cargo build --release --package codemod --bin codemod

# Test a command
./target/release/codemod workflow validate -w path/to/workflow.yaml
./target/release/codemod jssg run script.js --target ./test-dir
```

## Submitting Changes

### Before Submitting

1. **Update documentation** if you've changed functionality
2. **Add changelog entries** if applicable
3. **Ensure all tests pass**
4. **Ensure linting passes**
5. **Rebase on latest main**:
   ```bash
   git fetch upstream
   git rebase upstream/main
   ```

### Commit Messages

We use [Conventional Commits](https://www.conventionalcommits.org/):

- `feat:` for new features
- `fix:` for bug fixes
- `docs:` for documentation changes
- `refactor:` for code refactoring
- `test:` for test changes
- `chore:` for maintenance tasks
- `perf:` for performance improvements

Examples:
```
feat(cli): add support for custom workflow paths
fix(jssg): handle edge case in TypeScript parsing
docs: update CLI reference with new commands
```

### Pull Request Process

1. **Push your branch** to your fork:
   ```bash
   git push origin feature/your-feature-name
   ```

2. **Create a Pull Request** on GitHub:
   - Use a clear, descriptive title
   - Fill out the PR template
   - Link any related issues
   - Describe what changes you made and why
   - Include screenshots for UI changes

3. **Respond to feedback**:
   - Address review comments
   - Make requested changes
   - Keep the discussion constructive

4. **Sign the CLA**: You'll be asked to sign our Contributor License Agreement when you create your first PR.

### PR Checklist

- [ ] Code follows the project's style guidelines
- [ ] Tests pass locally
- [ ] Documentation updated (if needed)
- [ ] Commit messages follow Conventional Commits
- [ ] Changes are focused and atomic
- [ ] No breaking changes (or clearly documented)

## Contributing to Specific Areas

### CLI Development

The CLI is written in Rust. Key areas:

- **Commands** (`crates/cli/src/commands/`) – Add new commands or improve existing ones
- **Workflow execution** (`crates/cli/src/workflow_runner.rs`) – Workflow execution logic
- **Engine integration** (`crates/cli/src/engine.rs`) – Integration with execution engines

### Workflow Engine

The workflow engine (`crates/core/`) orchestrates multi-step codemod execution:

- **Execution logic** – How workflows are parsed and executed
- **Step types** – Support for new step types
- **Error handling** – Better error messages and recovery

### jssg

JavaScript ast-grep execution (`crates/codemod-sandbox/`):

- **Execution engine** – JavaScript/TypeScript code execution
- **Module resolution** – Better handling of imports and dependencies
- **TypeScript support** – Enhanced TypeScript parsing and transformation

### Documentation

Documentation is in `docs/`:

- **MDX files** – Documentation content
- **Examples** – Code examples and snippets
- **Images** – Screenshots and diagrams

### Website

The marketing website is in `apps/frontend/`:

- **Next.js app** – React components and pages
- **Styling** – Tailwind CSS
- **Content** – Sanity CMS integration

## Getting Help

- **Documentation**: [docs.codemod.com](https://docs.codemod.com)
- **Community Slack**: [codemod.com/community](https://codemod.com/community)
- **GitHub Issues**: For bug reports and feature requests

## License

By contributing, you agree that your contributions will be licensed under the Apache License 2.0.

## Thank You! 🎉

Thank you for contributing to Codemod! Your contributions help make code modernization accessible to everyone. Whether you're fixing a typo, adding a feature, or publishing a codemod to the registry, every contribution makes a difference.
