#!/usr/bin/env bash
# Interleaved rounds: R rounds; each round shuffles the arm order (seeded) and
# runs every arm ONCE with `hyperfine -N --runs 1`, the repository reset in
# --prepare, and CODEMOD_TIMINGS_FILE set for that run. One untimed warmup run
# of every arm precedes round 1. Interleaving spreads slow drift (thermal,
# background load) across arms instead of letting it land on one of them.
#
#   BENCH_ROUNDS=10 BENCH_CACHE=warm ./rounds.sh
#
# Environment (plus everything lib.sh reads):
#   BENCH_ROUNDS      rounds (default 10)
#   BENCH_CACHE       warm (default) or cold: cold wipes CODEMOD_WORKFLOW_CACHE (default
#                     <out>/workflow-cache in cold mode) before every run
#   BENCH_PURGE=1     also `sudo -n purge` before every run (needs cached sudo credentials)
#   BENCH_SEED        shuffle seed (default 1)
#   BENCH_SIZE        size label recorded in meta.json (default vite)
#   BENCH_OUT         output directory (default results/runs/<utc stamp>/<size>)
#   BENCH_CHECK_RUNS  1 (default): after every timed run, require the gate's diff hash
#   BENCH_PREFLIGHT_FILE  reuse this preflight output instead of running preflight.sh
#   BENCH_NO_ANALYZE=1    skip analyze.mjs at the end (sizes.sh analyzes all sizes at once)
#   BENCH_REPORT_DIR      where analyze.mjs writes report.{md,json} (default results/)
#   BENCH_INPUT_TAG       input label for provenance (default "vite <tag>")
#
# Output: <out>/raw/<round>-<arm>.json (hyperfine export), .timings.json (+ child
# .timings.json.<pid>.json files), .log; <out>/meta.json (provenance); then
# analyze.mjs writes report.md/report.json (exit 2 when noisy, see analyze.mjs).
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"

ROUNDS="${BENCH_ROUNDS:-10}"
CACHE="${BENCH_CACHE:-warm}"
SEED="${BENCH_SEED:-1}"
SIZE="${BENCH_SIZE:-vite}"
OUT="${BENCH_OUT:-$RESULTS_DIR/runs/$(date -u +%Y%m%dT%H%M%SZ)/$SIZE}"
case "$CACHE" in warm | cold) ;; *) die "BENCH_CACHE must be warm or cold, not $CACHE" ;; esac
case "$ROUNDS" in '' | *[!0-9]*) die "BENCH_ROUNDS must be a positive integer" ;; esac

record_start_state
require_hyperfine
mkdir -p "$OUT/raw" "$OUT/logs"
OUT="$(cd "$OUT" && pwd)"

if [ -n "${BENCH_PREFLIGHT_FILE:-}" ]; then
  cp "$BENCH_PREFLIGHT_FILE" "$OUT/preflight.txt"
else
  set +e
  bash "$BENCH_DIR/preflight.sh" | tee "$OUT/preflight.txt" >&2
  pf="${PIPESTATUS[0]}"
  set -e
  [ "$pf" -eq 0 ] || die "preflight failed (see above; BENCH_PREFLIGHT=warn to run anyway)"
fi

bash "$BENCH_DIR/verify.sh"
# shellcheck source=/dev/null
. "$RESULTS_DIR/arms.env"
ensure_bench_exec

if [ "$CACHE" = "cold" ]; then
  CODEMOD_WORKFLOW_CACHE="${CODEMOD_WORKFLOW_CACHE:-$OUT/workflow-cache}"
  export CODEMOD_WORKFLOW_CACHE
fi
if [ "${BENCH_PURGE:-0}" = "1" ]; then
  sudo -n true 2>/dev/null || die "BENCH_PURGE=1 needs cached sudo credentials (run 'sudo -v' first)"
fi

# prepare.sh <timings-file>: what hyperfine runs before every timed run.
{
  printf '#!/usr/bin/env bash\nset -euo pipefail\n'
  printf 'git -C %s reset -q --hard\ngit -C %s clean -fdq\n' "$(q "$REPO")" "$(q "$REPO")"
  printf 'if [ -n "${1:-}" ]; then rm -f "$1" "$1".*.json; fi\n'
  if [ "$CACHE" = "cold" ]; then printf 'rm -rf %s\n' "$(q "$CODEMOD_WORKFLOW_CACHE")"; fi
  if [ "${BENCH_PURGE:-0}" = "1" ]; then printf 'sudo -n purge\n'; fi
} >"$OUT/prepare.sh"

: >"$OUT/arms.tsv"
for arm in $AVAILABLE_ARMS; do
  printf '%s\t%s\t%s\n' "$arm" "$(arm_command "$arm")" "$(arm_concurrency "$arm")" >>"$OUT/arms.tsv"
done

files="$(git -C "$REPO" ls-files -- '*.ts' '*.mts' '*.cts' '*.js' '*.mjs' '*.cjs' | grep -vc 'node_modules/' || true)"

trap 'reset_repo "$REPO"' EXIT

log "size $SIZE: $files candidate files, gate $GATE_FILES files ($GATE_HASH)"
log "warmup: one untimed run per arm (cache: $CACHE)"
for arm in $AVAILABLE_ARMS; do
  bash "$OUT/prepare.sh"
  env -u CODEMOD_TIMINGS_FILE bash -c "$(arm_command "$arm")" >"$OUT/logs/warmup-$arm.log" 2>&1 ||
    die "warmup of $arm failed (log: $OUT/logs/warmup-$arm.log)"
done

for round in $(seq 1 "$ROUNDS"); do
  order="$(shuffle "$((SEED * 1000 + round))" $AVAILABLE_ARMS)"
  line=""
  for arm in $order; do
    base="$OUT/raw/$round-$arm"
    bench_once "$base.json" "$arm" "bash $(q "$OUT/prepare.sh") $(q "$base.timings.json")" \
      "$base.timings.json" "$(arm_command "$arm")"
    if [ "${BENCH_CHECK_RUNS:-1}" = "1" ]; then
      got="$(diff_hash "$REPO")"
      [ "$got" = "$GATE_HASH" ] || die "round $round, $arm: diff hash $got differs from the gate's $GATE_HASH"
    fi
    line="$line $arm=$(hyperfine_time "$base.json")s"
  done
  log "round $round/$ROUNDS:$line"
done

BENCH_INPUT_TAG="${BENCH_INPUT_TAG:-vite $VITE_TAG}" node "$BENCH_DIR/provenance.mjs" --out "$OUT" --size "$SIZE" --files "$files" --rounds "$ROUNDS" \
  --cache "$CACHE" --seed "$SEED" --repo "$REPO" --gate-hash "$GATE_HASH" --gate-files "$GATE_FILES" \
  --purge "${BENCH_PURGE:-0}" >"$OUT/meta.json"
log "raw results: $OUT"

if [ "${BENCH_NO_ANALYZE:-0}" != "1" ]; then
  node "$BENCH_DIR/analyze.mjs" "$OUT" --out "${BENCH_REPORT_DIR:-$RESULTS_DIR}" >/dev/null
fi
