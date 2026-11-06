# shellcheck shell=bash
# Shared configuration for setup.sh, verify.sh, and run.sh. Sourced, not run.
#
# Environment overrides:
#   BENCH_REPO           vite checkout (default: $HOME/.cache/codemod-bench/vite)
#   CODEMOD_BIN          release `codemod` CLI (default: <cargo target>/release/codemod)
#   CODEMOD_BRIDGE_BIN   release bridge (default: <cargo target>/release/butterflow-execution-bridge)
#   CARGO_TARGET_DIR     honoured when locating the two binaries (default: <repo>/target)
#   BENCH_HOME           if set, HOME for every arm (isolates engine state and git config)
#   BENCH_ARMS           space-separated arms to consider (default: "yaml ts-bridge ts-node ts-quickjs")
#   BENCH_REQUIRE_ARMS   space-separated arms that must be available (default: "yaml ts-bridge")
#   BENCH_CMD_<ARM>      replace an arm's command line, e.g. BENCH_CMD_TS_QUICKJS='...'.
#                        Evaluated by bash with $REPO and the variables below in scope.

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
CODEMOD_BIN="${CODEMOD_BIN:-$TARGET_DIR/release/codemod}"
CODEMOD_BRIDGE_BIN="${CODEMOD_BRIDGE_BIN:-$TARGET_DIR/release/butterflow-execution-bridge}"
WORKFLOW_CLI="$ROOT/packages/orchestration/bin/codemod-workflow.mjs"
YAML_WORKFLOW="$BENCH_DIR/yaml/workflow.yaml"
TS_WORKFLOW="$BENCH_DIR/ts/workflow.ts"
export CODEMOD_BRIDGE_BIN

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

ALL_ARMS="yaml ts-bridge ts-node ts-quickjs"
BENCH_ARMS="${BENCH_ARMS:-$ALL_ARMS}"
BENCH_REQUIRE_ARMS="${BENCH_REQUIRE_ARMS:-yaml ts-bridge}"

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
  local head
  head="$(git -C "$REPO" rev-parse HEAD)"
  [ "$head" = "$VITE_SHA" ] || die "$REPO is at $head, expected $VITE_SHA ($VITE_TAG); rerun setup.sh"
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
    die "codemod CLI not found at $CODEMOD_BIN; run 'cargo build --release -p codemod -p butterflow-execution-bridge'"
  [ -x "$CODEMOD_BRIDGE_BIN" ] ||
    die "bridge not found at $CODEMOD_BRIDGE_BIN; run 'cargo build --release -p codemod -p butterflow-execution-bridge'"
  command -v node >/dev/null || die "node is required"
}

arm_var() { printf 'BENCH_CMD_%s' "$(printf '%s' "$1" | tr 'a-z-' 'A-Z_')"; }

# The command line for an arm, run with `bash -c` (hyperfine uses the same).
# `ts-bridge` uses the `run --runtime=bridge` form once the CLI has it and the
# legacy `codemod-workflow <workflow.ts>` form until then (see probe_arm).
q() { printf '%q' "$1"; }
arm_command() {
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
    ts-bridge)
      if [ "${TS_BRIDGE_FORM:-legacy}" = "run" ]; then
        printf 'node %s run --runtime=bridge %s --target %s --bridge %s' \
          "$(q "$WORKFLOW_CLI")" "$(q "$TS_WORKFLOW")" "$(q "$repo")" "$(q "$CODEMOD_BRIDGE_BIN")"
      else
        printf 'node %s %s --target %s --bridge %s' \
          "$(q "$WORKFLOW_CLI")" "$(q "$TS_WORKFLOW")" "$(q "$repo")" "$(q "$CODEMOD_BRIDGE_BIN")"
      fi
      ;;
    ts-node | ts-quickjs)
      printf 'node %s run --runtime=%s %s --target %s' \
        "$(q "$WORKFLOW_CLI")" "${arm#ts-}" "$(q "$TS_WORKFLOW")" "$(q "$repo")"
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
    ts-bridge)
      printf 'sequence(): one command at a time; each jssg command is one bridge process transforming its batch serially on 1 thread, one QuickJS runtime per file (file selection/reads/writes in Node)'
      ;;
    ts-node)
      printf 'one Node process; workflow and every transform on the single JS thread, serially per file (Rust may parse/match off-thread)'
      ;;
    ts-quickjs)
      printf 'one QuickJS context in one Rust process; workflow and every transform on the single JS thread, serially per file (Rust may parse/match off-thread)'
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
# expected edit. Sets TS_BRIDGE_FORM for ts-bridge. Diagnostics go to
# $PROBE_LOG.
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
  if [ "$arm" = "ts-bridge" ] && [ -z "${BENCH_CMD_TS_BRIDGE:-}" ]; then
    if TS_BRIDGE_FORM=run try; then
      TS_BRIDGE_FORM=run rc=0
    elif TS_BRIDGE_FORM=legacy try; then
      TS_BRIDGE_FORM=legacy rc=0
    fi
    export TS_BRIDGE_FORM
  elif try; then
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
