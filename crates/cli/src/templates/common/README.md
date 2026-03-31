# {name}

{description}

## Installation

```bash
# Install from registry
codemod run {name}

# Or run locally
codemod workflow run -w workflow.yaml --target <repo-path>
```

## Usage

This codemod transforms {language} code by:

- Converting `var` declarations to `const`/`let`
- Removing debug statements
- Modernizing syntax patterns

## Development

```bash
# Test the transformation
{test_command}

# Validate the workflow
codemod workflow validate -w workflow.yaml

# Publish to registry
codemod login
codemod publish
```

## License

{license} 
