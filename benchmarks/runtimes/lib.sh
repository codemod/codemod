# shellcheck shell=bash
# Shared configuration for every harness script (setup, verify, run, preflight,
# rounds, sizes, overhead). Sourced, not run.
#
# Environment overrides:
#   BENCH_REPO           vite checkout (default: $HOME/.cache/codemod-bench/vite)
#   CODEMOD_BIN          release `codemod` CLI (default: <cargo target>/release/codemod)
#   CARGO_TARGET_DIR     honoured when locating the binaries (default: <repo>/target)
#   BENCH_HOME           if set, HOME for every arm (isolates engine state and git config)
#   BENCH_ARMS           space-separated arms to consider (default: "yaml ts-quickjs")
#   BENCH_REQUIRE_ARMS   space-separated arms that must be available (default: "yaml ts-quickjs")
#   BENCH_CMD_<ARM>      replace an arm's command line, e.g. BENCH_CMD_TS_QUICKJS='...'.
#                        Evaluated by bash with $REPO and the variables below in scope.
#   BENCH_TIMEOUT        if set, seconds after which any single arm run is killed (SIGALRM,
#                        armed by bench-exec before it execs the arm), so a hung run fails
#                        the gate or the benchmark instead of stalling it
#   BENCH_EXPECT_SHA     commit the checkout must be at (default: the pinned vite commit;
#                        sizes.sh sets it for its scaled repositories)
#   CODEMOD_QUICKJS_HOST_BIN  release quickjs host (default: <cargo target>/release/...)
#
# Every arm is launched through bin/bench-exec (compiled from bench-exec.c on
# first use), which sets BENCH_T0_NS and execs the arm in the same process.

set -euo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$BENCH_DIR/../.." && pwd)"
RESULTS_DIR="$BENCH_DIR/results"

# Pinned benchmark input: vitejs/vite release tag and the commit it points to.
VITE_URL="https://github.com/vitejs/vite.git"
VITE_TAG="v8.3.6"
VITE_SHA="0fd9a29f57d49d496f39c952f44c1b5fc3e90769"

REPO="${BENCH_REPO:-$HOME/.cache/codemod-bench/vite}"

TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
case "$TARGET_DIR" in /*) ;; *) TARGET_DIR="$ROOT/$TARGET_DIR" ;; esac
if [ -d "$TARGET_DIR" ]; then TARGET_DIR="$(cd "$TARGET_DIR" && pwd)"; fi
CODEMOD_BIN="${CODEMOD_BIN:-$TARGET_DIR/release/codemod}"
WORKFLOW_CLI="$ROOT/packages/orchestration/bin/codemod-workflow.mjs"
YAML_WORKFLOW="$BENCH_DIR/yaml/workflow.yaml"
TS_WORKFLOW="$BENCH_DIR/ts/workflow.ts"
# Pin the release host explicitly (the runtime would otherwise pick the newer
# of release/debug).
CODEMOD_QUICKJS_HOST_BIN="${CODEMOD_QUICKJS_HOST_BIN:-$TARGET_DIR/release/orchestration-quickjs-host}"
export CODEMOD_QUICKJS_HOST_BIN
# Read by provenance.mjs.
export CODEMOD_BIN VITE_TAG VITE_SHA

BIN_DIR="$BENCH_DIR/bin"
BENCH_EXEC="$BIN_DIR/bench-exec"

# No telemetry from either engine.
export DISABLE_ANALYTICS=true

# Optional: run every arm with HOME pointed at a scratch directory. The engine
# keeps run state under the platform data dir (~/Library/Application Support/
# butterflow on macOS), and both walkers honour the global git excludes file,
# so an isolated HOME makes runs independent of the user's profile (and works
# where the real data dir is not writable).
if [ -n "${BENCH_HOME:-}" ]; then
  mkdir -p "$BENCH_HOME"
  HOME="$(cd "$BENCH_HOME" && pwd)"
  export HOME
fi

ALL_ARMS="yaml ts-quickjs"
BENCH_ARMS="${BENCH_ARMS:-$ALL_ARMS}"
BENCH_REQUIRE_ARMS="${BENCH_REQUIRE_ARMS:-yaml ts-quickjs}"

now() { perl -MTime::HiRes=time -e 'printf "%.3f\n", time'; }
log() { printf '[bench] %s\n' "$*" >&2; }
die() {
  printf '[bench] error: %s\n' "$*" >&2
  exit 1
}

# The checkout must exist at the pinned commit. It is only ever read, reset,
# and cleaned by git; nothing inside it is executed.
require_repo() {
  [ -d "$REPO/.git" ] || die "no vite checkout at $REPO; run setup.sh first (or set BENCH_REPO)"
  local head expected="${BENCH_EXPECT_SHA:-$VITE_SHA}"
  head="$(git -C "$REPO" rev-parse HEAD)"
  [ "$head" = "$expected" ] || die "$REPO is at $head, expected $expected; rerun setup.sh (or sizes.sh)"
}

# Record the codemod worktree's commit and dirty flag once, when a run starts
# (exported, so a sizes.sh session records one state for every size).
record_start_state() {
  if [ -z "${BENCH_START_SHA:-}" ]; then
    BENCH_START_SHA="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"
    BENCH_START_DIRTY=false
    [ -z "$(git -C "$ROOT" status --porcelain 2>/dev/null)" ] || BENCH_START_DIRTY=true
    export BENCH_START_SHA BENCH_START_DIRTY
  fi
}

# Compile bench-exec.c into bin/ when missing or older than its source.
ensure_bench_exec() {
  if [ ! -x "$BENCH_EXEC" ] || [ "$BENCH_DIR/bench-exec.c" -nt "$BENCH_EXEC" ]; then
    command -v cc >/dev/null || die "a C compiler (cc) is required to build bench-exec"
    mkdir -p "$BIN_DIR"
    cc -O2 -Wall -o "$BENCH_EXEC.tmp.$$" "$BENCH_DIR/bench-exec.c" ||
      die "failed to compile bench-exec.c"
    mv -f "$BENCH_EXEC.tmp.$$" "$BENCH_EXEC"
  fi
}

# Release binary an arm needs, for preflight and provenance.
arm_binary() {
  case "$1" in
    yaml) printf '%s' "$CODEMOD_BIN" ;;
    ts-quickjs) printf '%s' "$CODEMOD_QUICKJS_HOST_BIN" ;;
  esac
}

# Fisher-Yates shuffle of the remaining arguments, seeded (reproducible).
shuffle() {
  local seed="$1"
  shift
  perl -MList::Util=shuffle -e 'srand(shift); print join(" ", shuffle @ARGV), "\n"' "$seed" "$@"
}

# Locate hyperfine (PATH or ~/.cargo/bin); dies when it is missing.
require_hyperfine() {
  if ! command -v hyperfine >/dev/null; then
    if [ -x "$HOME/.cargo/bin/hyperfine" ]; then
      PATH="$HOME/.cargo/bin:$PATH"
    else
      die "hyperfine is required (brew install hyperfine, or cargo install hyperfine)"
    fi
  fi
}

# Hash of the repository's full change set (staged, binary), as verify.sh computes it.
diff_hash() {
  git -C "$1" add -A
  git -C "$1" diff --cached --binary | shasum -a 256 | cut -d' ' -f1
}

# bench_once <out.json> <name> <prepare-cmd> <timings-file|""> <arm-cmd>
# One timed run through hyperfine without a shell (-N): the timed process is
# bench-exec, which execs the arm. CODEMOD_TIMINGS_FILE is set only when a
# timings path is given. hyperfine's own output goes to <out.json%.json>.log.
bench_once() {
  local out="$1" name="$2" prepare="$3" timings="$4" cmd="$5"
  local log="${out%.json}.log"
  if [ -n "$timings" ]; then
    CODEMOD_TIMINGS_FILE="$timings" BENCH_ARM="$name" hyperfine -N --runs 1 --style basic \
      --prepare "$prepare" --export-json "$out" -n "$name" "$cmd" >"$log" 2>&1
  else
    env -u CODEMOD_TIMINGS_FILE BENCH_ARM="$name" hyperfine -N --runs 1 --style basic \
      --prepare "$prepare" --export-json "$out" -n "$name" "$cmd" >"$log" 2>&1
  fi || {
    tail -n 30 "$log" >&2
    die "run $name failed (log: $log)"
  }
}

# Wall time (s) of the single run in a hyperfine export.
hyperfine_time() {
  awk -F': *' '/"mean"/ { gsub(/[ ,]/, "", $2); print $2; exit }' "$1"
}

reset_repo() {
  git -C "$1" reset -q --hard
  git -C "$1" clean -fdq
}

# `ts/workflow.ts` imports `@codemod.com/orchestration` by package name; the
# benchmark directory is not a workspace package, so link it (git-ignored).
ensure_ts_link() {
  [ -d "$ROOT/packages/orchestration/node_modules" ] ||
    die "packages/orchestration has no node_modules; run 'pnpm install' at the repo root first"
  local dir="$BENCH_DIR/ts/node_modules/@codemod.com"
  mkdir -p "$dir"
  if [ ! -L "$dir/orchestration" ]; then
    ln -s ../../../../../packages/orchestration "$dir/orchestration"
  fi
}

# Bundle-root symlink for the YAML workflow: js_file must be a relative path
# inside the workflow directory. Committed to git; recreated if a checkout
# (e.g. core.symlinks=false) dropped it.
ensure_yaml_link() {
  local link="$BENCH_DIR/yaml/transforms"
  if [ ! -L "$link" ] && [ ! -d "$link" ]; then
    ln -s ../transforms "$link"
  fi
}

require_binaries() {
  [ -x "$CODEMOD_BIN" ] ||
    die "codemod CLI not found at $CODEMOD_BIN; run 'cargo build --release -p codemod'"
  command -v node >/dev/null || die "node is required"
}

arm_var() { printf 'BENCH_CMD_%s' "$(printf '%s' "$1" | tr 'a-z-' 'A-Z_')"; }

# The command line for an arm, run with `bash -c` (hyperfine uses the same).
q() { printf '%q' "$1"; }
# Every arm starts as bin/bench-exec, which sets BENCH_T0_NS and execs the arm
# (same process, so the arm itself is what hyperfine waits on). With
# BENCH_TIMEOUT it arms an alarm first and SIGALRM ends the arm (exit 142).
# The result is a plain command line (no shell syntax), so it also runs under
# `hyperfine -N`; a BENCH_CMD_<ARM> override must be plain too (or `bash -c '...'`).
arm_command() {
  ensure_bench_exec
  printf '%s ' "$(q "$BENCH_EXEC")"
  if [ -n "${BENCH_TIMEOUT:-}" ]; then
    printf -- '-t %s ' "$(q "$BENCH_TIMEOUT")"
  fi
  arm_command_unbounded "$@"
}
arm_command_unbounded() {
  local arm="$1" repo="${2:-$REPO}" override
  override="$(arm_var "$arm")"
  if [ -n "${!override:-}" ]; then
    local REPO="$repo"
    eval "printf '%s' \"${!override}\""
    return
  fi
  case "$arm" in
    yaml)
      printf '%s workflow run --workflow %s --target %s --no-interactive --disable-analytics' \
        "$(q "$CODEMOD_BIN")" "$(q "$YAML_WORKFLOW")" "$(q "$repo")"
      ;;
    ts-quickjs)
      printf 'node %s run --runtime=quickjs %s --target %s' \
        "$(q "$WORKFLOW_CLI")" "$(q "$TS_WORKFLOW")" "$(q "$repo")"
      ;;
    *) die "unknown arm: $arm" ;;
  esac
}

# Thread/concurrency settings each arm runs with (all defaults; printed, not set).
arm_concurrency() {
  local cpus
  cpus="$(getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.ncpu)"
  case "$1" in
    yaml)
      printf 'js-ast-grep steps run one after another; each walks and transforms files on min(available_parallelism=%s, 12) = %s threads (no max_threads), one QuickJS runtime per file' \
        "$cpus" "$((cpus < 12 ? cpus : 12))"
      ;;
    ts-quickjs)
      printf 'one Rust process (launched by Node, which reuses the cached bundle); the workflow in one QuickJS context; a JSSG command of 64+ files on a transform pool of min(available_parallelism=%s, 4) = %s worker threads (CODEMOD_JSSG_WORKERS), one QuickJS context each; smaller commands serially on the JS thread (native walk/reads on worker threads)' \
        "$cpus" "$((cpus < 4 ? cpus : 4))"
      ;;
  esac
}

# Markdown list describing the machine and toolchain.
machine_spec() {
  if [ "$(uname -s)" = "Darwin" ]; then
    if sysctl -n hw.memsize >/dev/null 2>&1; then
      local mem
      mem="$(sysctl -n hw.memsize)"
      printf -- '- Model: %s\n' "$(sysctl -n hw.model)"
      printf -- '- CPU: %s, %s cores (%s performance + %s efficiency), %s logical\n' \
        "$(sysctl -n machdep.cpu.brand_string)" "$(sysctl -n hw.physicalcpu)" \
        "$(sysctl -n hw.perflevel0.physicalcpu 2>/dev/null || echo '?')" \
        "$(sysctl -n hw.perflevel1.physicalcpu 2>/dev/null || echo 0)" "$(sysctl -n hw.logicalcpu)"
      printf -- '- RAM: %s GiB\n' "$((mem / 1024 / 1024 / 1024))"
    else
      # sysctl can be denied inside sandboxes; system_profiler (only the
      # non-identifying fields) still reports the hardware.
      local hw
      hw="$(system_profiler SPHardwareDataType 2>/dev/null)"
      field() { printf '%s\n' "$hw" | awk -F': ' -v k="$1" '$1 ~ k {print $2; exit}'; }
      printf -- '- Model: %s (%s)\n' "$(field 'Model Identifier')" "$(field 'Model Name')"
      printf -- '- CPU: %s, %s\n' "$(field 'Chip')" "$(field 'Total Number of Cores')"
      printf -- '- RAM: %s\n' "$(field 'Memory')"
    fi
    printf -- '- OS: macOS %s (%s), %s\n' "$(sw_vers -productVersion)" "$(sw_vers -buildVersion)" "$(uname -m)"
  else
    printf -- '- CPU: %s, %s logical\n' \
      "$(grep -m1 'model name' /proc/cpuinfo 2>/dev/null | cut -d: -f2- | sed 's/^ //')" "$(nproc)"
    printf -- '- RAM: %s\n' "$(free -h 2>/dev/null | awk '/Mem:/ {print $2}')"
    printf -- '- OS: %s %s\n' "$(uname -sr)" "$(uname -m)"
  fi
  printf -- '- node: %s; codemod: %s; hyperfine: %s\n' "$(node --version)" \
    "$("$CODEMOD_BIN" --version 2>/dev/null | head -n1)" \
    "$({ command -v hyperfine >/dev/null && hyperfine --version; } || echo n/a)"
  printf -- '- codemod repo: %s; vite: %s (%s)\n' \
    "$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo '?')" "$VITE_TAG" "$VITE_SHA"
}

# Tiny scratch repository used to probe whether an arm can run at all. Its
# expected post-run diff is fixed, so a probe also catches an arm that runs
# but does nothing.
make_probe_repo() {
  local dir="$1"
  mkdir -p "$dir/src"
  cat >"$dir/src/a.ts" <<'EOF'
import { describe, expect } from 'vitest';
import { defineConfig } from "vitest/config";
console.log("a", 1);
EOF
  printf 'export const b = 1;\n' >"$dir/src/b.ts"
  git -C "$dir" init -q
  git -C "$dir" add -A
  git -C "$dir" -c user.name=bench -c user.email=bench@localhost -c commit.gpgsign=false \
    commit -q -m probe
}

probe_expected() {
  cat <<'EOF'
import { describe, expect } from '@acme/test';
import { defineConfig } from "vitest/config";
logger.info("a", 1);
EOF
}

# probe_arm <arm>: 0 when the arm runs on the probe repo and produces the
# expected edit. Diagnostics go to $PROBE_LOG.
probe_arm() {
  local arm="$1" scratch rc=1
  scratch="$(mktemp -d "${TMPDIR:-/tmp}/codemod-bench-probe.XXXXXX")"
  PROBE_LOG="$scratch.log"
  make_probe_repo "$scratch/repo"
  try() {
    reset_repo "$scratch/repo"
    bash -c "$(arm_command "$arm" "$scratch/repo")" >"$PROBE_LOG" 2>&1 &&
      [ "$(cat "$scratch/repo/src/a.ts")" = "$(probe_expected)" ]
  }
  if try; then
    rc=0
  fi
  rm -rf "$scratch"
  return "$rc"
}

# Probe every arm in BENCH_ARMS; sets AVAILABLE_ARMS. Arms in
# BENCH_REQUIRE_ARMS fail the script when unavailable; others are skipped.
detect_arms() {
  AVAILABLE_ARMS=""
  local arm
  for arm in $BENCH_ARMS; do
    if probe_arm "$arm"; then
      AVAILABLE_ARMS="$AVAILABLE_ARMS $arm"
      log "arm $arm: available"
      log "  command: $(arm_command "$arm")"
      log "  concurrency: $(arm_concurrency "$arm")"
    else
      case " $BENCH_REQUIRE_ARMS " in
        *" $arm "*)
          tail -n 20 "$PROBE_LOG" >&2 || true
          die "required arm $arm failed its probe (log above)"
          ;;
      esac
      log "arm $arm: SKIPPED (not available yet: its probe run failed; last lines below)"
      tail -n 3 "$PROBE_LOG" | sed 's/^/[bench]   | /' >&2 || true
    fi
    rm -f "$PROBE_LOG"
  done
  AVAILABLE_ARMS="${AVAILABLE_ARMS# }"
  [ -n "$AVAILABLE_ARMS" ] || die "no arm is available"
}
