#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

versions="$(
  cargo tree -p codemod --prefix none \
    | sed -n 's/^crossterm v\([0-9][0-9]*\)\..*/\1/p' \
    | sort -u
)"

count="$(printf '%s\n' "$versions" | sed '/^$/d' | wc -l | tr -d ' ')"

if [[ "$count" != "1" ]]; then
  echo "Expected exactly one crossterm major version in codemod's dependency graph."
  echo "Found majors:"
  printf '%s\n' "$versions"
  exit 1
fi

printf 'Using crossterm major version: %s\n' "$versions"
