#!/usr/bin/env bash
# Machine checks before a benchmark. Prints the machine spec block and one
# PASS/WARN/FAIL line per check, then exits 1 if any check failed. With
# BENCH_PREFLIGHT=warn, failures are reported as WARN and the exit is 0;
# BENCH_PREFLIGHT=off skips the checks (the spec block is still printed).
#
# Checks: AC power, Low Power Mode, thermal throttling (pmset CPU_Speed_Limit),
# 1-minute load average < BENCH_MAX_LOAD (default 2), no non-benchmark process
# over BENCH_MAX_PROC_CPU % CPU (default 20; offenders are listed), Spotlight
# not indexing, a clean codemod worktree (sha + dirty flag recorded either
# way), and release binaries present for every arm in BENCH_ARMS.
#
#   preflight.sh            # strict
#   BENCH_PREFLIGHT=warn preflight.sh
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"

MODE="${BENCH_PREFLIGHT:-strict}"
MAX_LOAD="${BENCH_MAX_LOAD:-2}"
MAX_PROC_CPU="${BENCH_MAX_PROC_CPU:-20}"
failed=0

pass() { printf 'PASS  %s\n' "$*"; }
warn() { printf 'WARN  %s\n' "$*"; }
fail() {
  if [ "$MODE" = "warn" ]; then
    printf 'WARN  %s (BENCH_PREFLIGHT=warn)\n' "$*"
  else
    printf 'FAIL  %s\n' "$*"
    failed=1
  fi
}
gt() { awk -v a="$1" -v b="$2" 'BEGIN { exit !(a + 0 > b + 0) }'; }

printf '## Machine\n\n'
machine_spec
printf -- '- rustc: %s\n' "$(rustc --version 2>/dev/null || echo n/a)"
dirty=false
[ -z "$(git -C "$ROOT" status --porcelain 2>/dev/null)" ] || dirty=true
printf -- '- codemod worktree: %s (%s), dirty=%s\n' \
  "$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo '?')" \
  "$(git -C "$ROOT" rev-parse --abbrev-ref HEAD 2>/dev/null || echo '?')" "$dirty"
printf -- '- bench repo: %s at %s\n' "$REPO" "$(git -C "$REPO" rev-parse HEAD 2>/dev/null || echo 'missing')"
printf '\n## Checks (mode: %s)\n\n' "$MODE"

if [ "$MODE" = "off" ]; then
  printf 'SKIP  all checks (BENCH_PREFLIGHT=off)\n'
  exit 0
fi

darwin=false
[ "$(uname -s)" = "Darwin" ] && darwin=true

# Power source.
if $darwin; then
  src="$(pmset -g batt 2>/dev/null | head -n1 || true)"
  case "$src" in
    *"AC Power"*) pass "power: AC" ;;
    *"Battery Power"*) fail "power: on battery ($src)" ;;
    *) warn "power: unknown (${src:-pmset unavailable})" ;;
  esac
  # Older macOS: lowpowermode 0/1. Newer: powermode 0 (automatic), 1 (low power), 2 (high power).
  lpm="$(pmset -g 2>/dev/null | awk '$1 == "lowpowermode" || $1 == "powermode" { print $1 "=" $2; exit }' || true)"
  case "$lpm" in
    lowpowermode=1 | powermode=1) fail "Low Power Mode is on ($lpm)" ;;
    lowpowermode=0 | powermode=0 | powermode=2) pass "Low Power Mode: off ($lpm)" ;;
    *) warn "Low Power Mode: unknown (${lpm:-not reported by pmset})" ;;
  esac
  therm="$(pmset -g therm 2>/dev/null || true)"
  limit="$(printf '%s\n' "$therm" | awk -F'= *' '/CPU_Speed_Limit/ { print $2; exit }')"
  if [ -z "$limit" ]; then
    pass "thermal: no CPU speed limit recorded"
  elif [ "$limit" -lt 100 ]; then
    fail "thermal: CPU_Speed_Limit=$limit (< 100, throttled)"
  else
    pass "thermal: CPU_Speed_Limit=$limit"
  fi
else
  warn "power/thermal checks are macOS-only"
fi

# Load average.
if $darwin; then
  load1="$(sysctl -n vm.loadavg 2>/dev/null | awk '{ print $2 }' || true)"
else
  load1="$(cut -d' ' -f1 /proc/loadavg 2>/dev/null || true)"
fi
if [ -z "$load1" ]; then
  warn "load: unknown"
elif gt "$MAX_LOAD" "$load1"; then
  pass "load: 1-min $load1 (< $MAX_LOAD)"
else
  fail "load: 1-min $load1 (>= $MAX_LOAD)"
fi

# Busy processes, excluding this script's own ancestry and ps itself.
ancestors=" $$ "
pid=$$
while [ "$pid" -gt 1 ] 2>/dev/null; do
  pid="$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d ' ')"
  [ -n "$pid" ] || break
  ancestors="$ancestors$pid "
done
busy="$(ps -Ao pid=,pcpu=,comm= 2>/dev/null | awk -v max="$MAX_PROC_CPU" -v skip="$ancestors" '
  { pid = $1; cpu = $2; $1 = ""; $2 = ""; sub(/^ +/, "");
    if (cpu + 0 > max + 0 && index(skip, " " pid " ") == 0 && $0 !~ /(^|\/)ps$/)
      printf "      %6s %5.1f%%  %s\n", pid, cpu, $0 }')"
if [ -n "$busy" ]; then
  fail "processes over $MAX_PROC_CPU% CPU:"
  printf '%s\n' "$busy"
else
  pass "no other process over $MAX_PROC_CPU% CPU"
fi

# Spotlight: indexing shows up as busy mds/mdworker processes.
if $darwin; then
  md_cpu="$(ps -Ao pcpu=,comm= 2>/dev/null | awk '$2 ~ /(mds|mds_stores|mdworker|mdworker_shared|mdsync)$/ { s += $1 } END { printf "%.1f", s + 0 }')"
  md_status="$(mdutil -s "$REPO" 2>/dev/null | tail -n1 | sed 's/^[[:space:]]*//' || true)"
  if gt "$md_cpu" 5; then
    fail "Spotlight is indexing (mds/mdworker at ${md_cpu}% CPU; volume: ${md_status:-unknown})"
  else
    pass "Spotlight idle (mds/mdworker ${md_cpu}% CPU; volume: ${md_status:-unknown})"
  fi
fi

# Worktree state.
if $dirty; then
  fail "codemod worktree has uncommitted changes (results would not match a commit)"
else
  pass "codemod worktree clean"
fi

# Release binaries for every requested arm, plus the tools.
for arm in $BENCH_ARMS; do
  bin="$(arm_binary "$arm")"
  if [ -f "$bin" ]; then
    case "$bin" in
      */debug/*) fail "$arm: $bin is a debug build" ;;
      *) pass "$arm: $bin" ;;
    esac
  else
    fail "$arm: missing $bin (build release binaries, or drop the arm from BENCH_ARMS)"
  fi
done
for tool in node hyperfine cc git perl; do
  if command -v "$tool" >/dev/null || { [ "$tool" = hyperfine ] && [ -x "$HOME/.cargo/bin/hyperfine" ]; }; then
    pass "tool: $tool"
  else
    fail "tool: $tool not found"
  fi
done

if [ "$failed" -ne 0 ]; then
  printf '\npreflight FAILED (set BENCH_PREFLIGHT=warn to run anyway)\n'
  exit 1
fi
printf '\npreflight passed\n'
