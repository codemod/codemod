#!/usr/bin/env bash
# Pool rerun: ts arms on every Phase 1 cell (YAML rows carried over from
# perf/raw.csv), all arms on the debarrel cells TS could not run before, and
# yaml-signal again on the two heavy cells as a cross-session check.
set -u
cd "$(dirname "$0")"
OUT=${OUT:-/mnt/project-files/ts-orchestration-proof/perf-pool/raw.csv}
b() { python3 -I bench.py --config config.json --out "$OUT" --warmup 1 --timeout 1800 "$@"; }
TS=--only-arms=ts-node,ts-quickjs
b --runs 5 $TS --only-workflows shell-chain --only-repos probe
b --runs 5 $TS --only-workflows util-is --only-repos probe,bluebird,taxonomy,excalidraw
b --runs 3 $TS --only-workflows util-is --only-repos calcom
b --runs 5 $TS --only-workflows react-19 --only-repos probe,bluebird,react-bootstrap-v1,taxonomy,excalidraw
b --runs 5 $TS --only-workflows debarrel --only-repos taxonomy
b --runs 3 $TS --only-workflows debarrel --only-repos calcom
b --runs 5 $TS --only-workflows audit --only-repos taxonomy,excalidraw
b --runs 3 $TS --only-workflows audit --only-repos calcom
b --runs 5 --only-workflows debarrel --only-repos excalidraw,react-bootstrap-v1
OUT=/mnt/project-files/ts-orchestration-proof/perf-pool/yaml-recheck.csv b --runs 3 --only-arms=yaml-signal --only-workflows react-19 --only-repos excalidraw
OUT=/mnt/project-files/ts-orchestration-proof/perf-pool/yaml-recheck.csv b --runs 3 --only-arms=yaml-signal --only-workflows debarrel --only-repos calcom
echo ALL-DONE
