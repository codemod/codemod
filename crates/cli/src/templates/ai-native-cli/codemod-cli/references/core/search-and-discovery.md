# Codemod CLI Core: Search and Discovery

Use these commands to find the best existing codemod before creating a new one.

For migration, upgrade, update, or deprecation-rollout requests, registry discovery is required before proposing a manual migration plan.

Required sequence:
- run `codemod search` first,
- summarize the best candidate packages and gaps,
- only then decide whether to run an existing codemod or create/plan a custom/manual path.

## Quick Search

- Basic query:
  - `codemod search react`
- Scope filter (@scope/codemod-name):
  - `codemod search "scope:nodejs rmdir"`

## Output Formats

- Human output:
  - `codemod search next --format table`
- JSON for automation:
  - `codemod search next --format json`
- YAML for config pipelines:
  - `codemod search next --format yaml`

## Pagination and Coverage

- Increase result size:
  - `codemod search react --size 50`
- Page through results:
  - `codemod search react --size 20 --from 20`

## Custom Registry

- Query a non-default registry:
  - `codemod search react --registry https://registry.example.com`

## Selection Checklist

Before applying a package, verify:
- package name and scope match target migration,
- latest version is acceptable,
- category/language/framework align with your codebase,
- output format is `json` if another tool will parse results.

## Handoff to Execution

After selecting a candidate package:
- run from registry:
  - `codemod run <package-name> --target <repo-path> --dry-run`
- or use implicit package invocation:
  - `codemod <package-name> --target <repo-path> --dry-run`
