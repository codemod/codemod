#!/usr/bin/env bash
# Full benchmark plan. Appends to $OUT; logs land next to it in logs/.
set -u
cd "$(dirname "$0")"
OUT=${OUT:-/mnt/project-files/ts-orchestration-proof/perf/raw.csv}
b() { python3 -I bench.py --config config.json --out "$OUT" --warmup 1 --timeout 1800 "$@"; }
b --runs 5 --only-workflows shell-chain --only-repos probe
b --runs 5 --only-workflows util-is --only-repos probe,bluebird,taxonomy,excalidraw
b --runs 3 --only-workflows util-is --only-repos calcom
b --runs 5 --only-workflows react-19 --only-repos probe,bluebird,react-bootstrap-v1,taxonomy,excalidraw
b --runs 5 --only-workflows debarrel --only-repos taxonomy
b --runs 3 --only-workflows debarrel --only-repos calcom
b --runs 5 --only-workflows audit --only-repos taxonomy,excalidraw
b --runs 3 --only-workflows audit --only-repos calcom
echo ALL-DONE
