#!/usr/bin/env bash
# Run rounds.sh at several input sizes, then analyze them together (which
# adds the wall = fixed + per_file x N fit and the size plot).
#
#   BENCH_SIZES="probe vite vite-x4 vite-x16" BENCH_ROUNDS=10 ./sizes.sh
#
# Sizes (every one is a git repository, so verify.sh's gate works unchanged):
#   probe     the two-file scratch repository from make_probe_repo (lib.sh);
#             one file changes. Measures each arm's fixed cost.
#   vite      the pinned checkout itself ($BENCH_REPO).
#   vite-xN   one repository holding N copies of the pinned checkout's tracked
#             files, in copy-0/ ... copy-<N-1>/ (`git archive` of the pinned
#             commit into each), committed once with a fixed author and date
#             so the commit, and so BENCH_EXPECT_SHA, is reproducible. Each
#             copy keeps its own .gitignore files, so file selection and the
#             expected diff are N times vite's. Any N works (vite-x2, ...).
#
# Scaled repositories live in BENCH_SIZES_ROOT (default: <BENCH_REPO>-sizes)
# and are rebuilt only when missing or not at their expected commit. Disk:
# about N x the checkout's size (git stores each blob once).
#
# Output: results/runs/<stamp>/<size>/ per size (see rounds.sh), one shared
# preflight, and results/report.{md,json} (BENCH_REPORT_DIR to change).
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"

SIZES="${BENCH_SIZES:-probe vite vite-x4 vite-x16}"
SIZES_ROOT="${BENCH_SIZES_ROOT:-$REPO-sizes}"
SESSION="${BENCH_SESSION:-$RESULTS_DIR/runs/$(date -u +%Y%m%dT%H%M%SZ)}"

record_start_state
require_repo
mkdir -p "$SIZES_ROOT" "$SESSION"
SESSION="$(cd "$SESSION" && pwd)"

# Fixed identity and dates: scaled repositories get the same commit every time.
fixed_git_env() {
  export GIT_AUTHOR_NAME=bench GIT_AUTHOR_EMAIL=bench@localhost
  export GIT_COMMITTER_NAME=bench GIT_COMMITTER_EMAIL=bench@localhost
  export GIT_AUTHOR_DATE="2000-01-01T00:00:00Z" GIT_COMMITTER_DATE="2000-01-01T00:00:00Z"
}

# build_size <name> <dir>: create the repository for a size if needed.
build_size() {
  local name="$1" dir="$2" stamp
  stamp="$name from $VITE_SHA"
  if [ -d "$dir/.git" ] && [ "$(cat "$dir/.git/bench-size" 2>/dev/null)" = "$stamp" ]; then
    reset_repo "$dir"
    return
  fi
  log "building $name in $dir"
  rm -rf "$dir"
  mkdir -p "$dir"
  (
    fixed_git_env
    case "$name" in
      probe) make_probe_repo "$dir" ;;
      vite-x*)
        local n="${name#vite-x}" i
        case "$n" in '' | *[!0-9]*) die "bad size $name (want vite-x<N>)" ;; esac
        git init -q "$dir"
        for ((i = 0; i < n; i++)); do
          mkdir -p "$dir/copy-$i"
          git -C "$REPO" archive --format=tar "$VITE_SHA" | tar -x -C "$dir/copy-$i"
        done
        git -C "$dir" add -A
        git -C "$dir" -c commit.gpgsign=false commit -q -m "$stamp"
        ;;
      *) die "unknown size $name (probe, vite, vite-x<N>)" ;;
    esac
  )
  git -C "$dir" config core.hooksPath /dev/null
  git -C "$dir" config core.fsmonitor false
  printf '%s' "$stamp" >"$dir/.git/bench-size"
}

log "preflight"
set +e
bash "$BENCH_DIR/preflight.sh" | tee "$SESSION/preflight.txt" >&2
pf="${PIPESTATUS[0]}"
set -e
[ "$pf" -eq 0 ] || die "preflight failed (see above; BENCH_PREFLIGHT=warn to run anyway)"

for size in $SIZES; do
  if [ "$size" = "vite" ]; then
    dir="$REPO"
    expect="$VITE_SHA"
    tag="vite $VITE_TAG"
  else
    dir="$SIZES_ROOT/$size"
    build_size "$size" "$dir"
    expect="$(git -C "$dir" rev-parse HEAD)"
    tag="$size (vite $VITE_TAG)"
    [ "$size" = "probe" ] && tag="probe (make_probe_repo)"
  fi
  log "=== size $size: $dir"
  BENCH_REPO="$dir" BENCH_EXPECT_SHA="$expect" BENCH_SIZE="$size" BENCH_OUT="$SESSION/$size" \
    BENCH_NO_ANALYZE=1 BENCH_PREFLIGHT_FILE="$SESSION/preflight.txt" BENCH_INPUT_TAG="$tag" \
    bash "$BENCH_DIR/rounds.sh"
done

node "$BENCH_DIR/analyze.mjs" "$SESSION" --out "${BENCH_REPORT_DIR:-$RESULTS_DIR}" >/dev/null
