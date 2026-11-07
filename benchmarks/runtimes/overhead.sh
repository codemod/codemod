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
# Output: results/overhead/<stamp>/raw/<i>-<arm>-{off,on}.json (+ .timings.json for on).
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"

RUNS="${BENCH_OVERHEAD_RUNS:-10}"
SEED="${BENCH_SEED:-1}"
OUT="${BENCH_OUT:-$RESULTS_DIR/overhead/$(date -u +%Y%m%dT%H%M%SZ)}"

require_hyperfine
mkdir -p "$OUT/raw" "$OUT/logs"
OUT="$(cd "$OUT" && pwd)"

bash "$BENCH_DIR/verify.sh"
# shellcheck source=/dev/null
. "$RESULTS_DIR/arms.env"
export TS_BRIDGE_FORM
ensure_bench_exec

{
  printf '#!/usr/bin/env bash\nset -euo pipefail\n'
  printf 'git -C %s reset -q --hard\ngit -C %s clean -fdq\n' "$(q "$REPO")" "$(q "$REPO")"
  printf 'if [ -n "${1:-}" ]; then rm -f "$1" "$1".*.json; fi\n'
} >"$OUT/prepare.sh"

trap 'reset_repo "$REPO"' EXIT

for arm in $AVAILABLE_ARMS; do
  bash "$OUT/prepare.sh"
  env -u CODEMOD_TIMINGS_FILE bash -c "$(arm_command "$arm")" >"$OUT/logs/warmup-$arm.log" 2>&1 ||
    die "warmup of $arm failed (log: $OUT/logs/warmup-$arm.log)"
done

for i in $(seq 1 "$RUNS"); do
  for arm in $(shuffle "$((SEED * 1000 + i))" $AVAILABLE_ARMS); do
    for mode in $(shuffle "$((SEED * 7919 + i))" off on); do
      base="$OUT/raw/$i-$arm-$mode"
      timings=""
      [ "$mode" = "on" ] && timings="$base.timings.json"
      bench_once "$base.json" "$arm" "bash $(q "$OUT/prepare.sh") $(q "$timings")" "$timings" "$(arm_command "$arm")"
    done
  done
  log "overhead run $i/$RUNS done"
done

node "$BENCH_DIR/overhead.mjs" "$OUT"
