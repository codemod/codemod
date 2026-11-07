#!/usr/bin/env bash
# Quick path: wall-clock benchmark of the available arms in one hyperfine
# session. Runs verify.sh first (the correctness gate), then benchmarks exactly
# the arms it verified. For interleaved rounds, sizes, CIs, and phase tables
# use rounds.sh / sizes.sh (README "Rounds, sizes, and analysis").
#
#   BENCH_RUNS=5 BENCH_WARMUP=1 ./run.sh
#
# Results: results/benchmark.md (committed), results/benchmark.json
# (git-ignored), results/environment.md (machine spec, commands, concurrency).
# Installs nothing: hyperfine must already be installed.
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"

RUNS="${BENCH_RUNS:-5}"
WARMUP="${BENCH_WARMUP:-1}"

require_hyperfine

bash "$BENCH_DIR/verify.sh"
# shellcheck source=/dev/null
. "$RESULTS_DIR/arms.env"
export TS_BRIDGE_FORM

prepare="git -C $(q "$REPO") reset -q --hard && git -C $(q "$REPO") clean -fdq"
args=(
  --shell=bash
  --warmup "$WARMUP"
  --runs "$RUNS"
  --prepare "$prepare"
  --export-markdown "$RESULTS_DIR/benchmark.md"
  --export-json "$RESULTS_DIR/benchmark.json"
)

{
  printf '# Runtime benchmark environment\n\n'
  printf 'Recorded %s. hyperfine --warmup %s --runs %s.\n\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$WARMUP" "$RUNS"
  machine_spec
  printf '\n## Arms\n\n'
} >"$RESULTS_DIR/environment.md"

log "machine:"
machine_spec | sed 's/^/[bench]   /' >&2

for arm in $AVAILABLE_ARMS; do
  cmd="$(arm_command "$arm")"
  args+=(-n "$arm" "$cmd")
  {
    printf -- '- `%s`: %s\n' "$arm" "$(arm_concurrency "$arm")"
    printf '  ```sh\n  %s\n  ```\n' "$cmd"
  } >>"$RESULTS_DIR/environment.md"
done
for arm in $BENCH_ARMS; do
  case " $AVAILABLE_ARMS " in
    *" $arm "*) ;;
    *) printf -- '- `%s`: skipped (not available)\n' "$arm" >>"$RESULTS_DIR/environment.md" ;;
  esac
done

trap 'reset_repo "$REPO"' EXIT
hyperfine "${args[@]}"
log "results: $RESULTS_DIR/benchmark.md, $RESULTS_DIR/environment.md"
