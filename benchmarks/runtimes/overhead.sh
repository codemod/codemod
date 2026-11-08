#!/usr/bin/env bash
# Cost of the timings instrumentation: every available arm runs
# BENCH_OVERHEAD_RUNS times (default 10) with CODEMOD_TIMINGS_FILE unset and
# as many times with it set, interleaved (each pair in a seeded random
# order), then overhead.mjs prints median(on) / median(off) with a bootstrap
# 95 % CI per arm against the 1 % budget. Uses the same launcher, prepare
# step, and warmup as rounds.sh.
#
#   BENCH_OVERHEAD_RUNS=10 ./overhead.sh
#
# Build mode: with BENCH_OVERHEAD_BASELINE=<cargo target dir> holding a
# release build *without* the `timings` feature (codemod, the quickjs host,
# and libjssg_napi), "off" runs that build and "on" runs the
# default one, both with CODEMOD_TIMINGS_FILE unset: the cost of compiling
# the instrumentation in, rather than of turning it on.
#
#   cargo build --release --target-dir /tmp/off -p codemod -p orchestration-quickjs-host \
#     -p jssg-napi
#   BENCH_OVERHEAD_BASELINE=/tmp/off ./overhead.sh
#
# Output: results/overhead/<stamp>/raw/<i>-<arm>-{off,on}.json (+ .timings.json
# for on, env mode only); <stamp>/mode (env or build).
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"

RUNS="${BENCH_OVERHEAD_RUNS:-10}"
SEED="${BENCH_SEED:-1}"
OUT="${BENCH_OUT:-$RESULTS_DIR/overhead/$(date -u +%Y%m%dT%H%M%SZ)}"

require_hyperfine
mkdir -p "$OUT/raw" "$OUT/logs"
OUT="$(cd "$OUT" && pwd)"

BASELINE="${BENCH_OVERHEAD_BASELINE:-}"
MODE=env
if [ -n "$BASELINE" ]; then
  MODE=build
  BASELINE="$(cd "$BASELINE" && pwd)"
  for bin in codemod orchestration-quickjs-host; do
    [ -x "$BASELINE/release/$bin" ] || die "baseline build lacks $BASELINE/release/$bin"
  done
  lib="$BASELINE/release/libjssg_napi.$([ "$(uname -s)" = Darwin ] && echo dylib || echo so)"
  [ -f "$lib" ] || die "baseline build lacks $lib"
  # Node loads addons by the .node extension only.
  BASELINE_NAPI="$OUT/baseline-jssg-napi.node"
  cp "$lib" "$BASELINE_NAPI"
fi
printf '%s\n' "$MODE" >"$OUT/mode"
log "overhead mode: $MODE${BASELINE:+ (baseline $BASELINE)}"

# arm_for <mode> <arm>: the arm's command line for one mode (build mode's
# "off" points every binary at the baseline build).
arm_for() {
  if [ "$MODE" = build ] && [ "$1" = off ]; then
    CODEMOD_BIN="$BASELINE/release/codemod" arm_command "$2"
  else
    arm_command "$2"
  fi
}
# run_env <mode> <cmd...>: the environment the mode's binaries need.
run_env() {
  local mode="$1"
  shift
  if [ "$MODE" = build ] && [ "$mode" = off ]; then
    CODEMOD_QUICKJS_HOST_BIN="$BASELINE/release/orchestration-quickjs-host" \
      CODEMOD_JSSG_NAPI="$BASELINE_NAPI" "$@"
  else
    "$@"
  fi
}

bash "$BENCH_DIR/verify.sh"
# shellcheck source=/dev/null
. "$RESULTS_DIR/arms.env"
ensure_bench_exec

{
  printf '#!/usr/bin/env bash\nset -euo pipefail\n'
  printf 'git -C %s reset -q --hard\ngit -C %s clean -fdq\n' "$(q "$REPO")" "$(q "$REPO")"
  printf 'if [ -n "${1:-}" ]; then rm -f "$1" "$1".*.json; fi\n'
} >"$OUT/prepare.sh"

trap 'reset_repo "$REPO"' EXIT

for arm in $AVAILABLE_ARMS; do
  for mode in off on; do
    bash "$OUT/prepare.sh"
    run_env "$mode" env -u CODEMOD_TIMINGS_FILE bash -c "$(arm_for "$mode" "$arm")" \
      >"$OUT/logs/warmup-$arm-$mode.log" 2>&1 ||
      die "warmup of $arm ($mode) failed (log: $OUT/logs/warmup-$arm-$mode.log)"
  done
done

for i in $(seq 1 "$RUNS"); do
  for arm in $(shuffle "$((SEED * 1000 + i))" $AVAILABLE_ARMS); do
    for mode in $(shuffle "$((SEED * 7919 + i))" off on); do
      base="$OUT/raw/$i-$arm-$mode"
      timings=""
      [ "$MODE" = env ] && [ "$mode" = "on" ] && timings="$base.timings.json"
      run_env "$mode" bench_once "$base.json" "$arm" "bash $(q "$OUT/prepare.sh") $(q "$timings")" \
        "$timings" "$(arm_for "$mode" "$arm")"
    done
  done
  log "overhead run $i/$RUNS done"
done

node "$BENCH_DIR/overhead.mjs" "$OUT"
