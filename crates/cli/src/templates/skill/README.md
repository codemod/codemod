# {name}

{description}

## Installation

```bash
# Install this package skill into the current project harness context
npx codemod@latest {name} --skill --project
```

## Usage

This package is skill-oriented and includes `workflow.yaml` with an `install-skill` step.
After installation, reload your harness session so the new skill is discoverable.

## Development

```bash
# Install skill wiring
{test_command}

# Publish to registry
codemod login
codemod publish
```

## License

{license}
