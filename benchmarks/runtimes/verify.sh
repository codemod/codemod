#!/usr/bin/env bash
# Correctness gate: run every available arm once on a clean vite checkout and
# require byte-identical results. For each arm the repository is reset and
# cleaned, the arm runs, every change (including new and deleted files) is
# staged, and `git diff --cached --binary` is hashed. All hashes must match.
#
# Writes results/arms.env (the arms run.sh benchmarks) and, per arm,
# results/verify/<arm>.{diff,files,log}. Exit status 1 on any mismatch.
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"

require_repo
require_binaries
ensure_ts_link
ensure_yaml_link
detect_arms

out="$RESULTS_DIR/verify"
rm -rf "$out"
mkdir -p "$out"

trap 'reset_repo "$REPO"' EXIT

first=""
status=0
printf '%-11s %8s %9s  %s\n' arm files seconds "sha256(git diff --binary)" >&2
for arm in $AVAILABLE_ARMS; do
  reset_repo "$REPO"
  start="$(now)"
  if ! bash -c "$(arm_command "$arm")" >"$out/$arm.log" 2>&1; then
    tail -n 30 "$out/$arm.log" >&2
    die "arm $arm failed (full log: $out/$arm.log)"
  fi
  end="$(now)"
  git -C "$REPO" add -A
  git -C "$REPO" diff --cached --binary >"$out/$arm.diff"
  # status <TAB> resulting blob <TAB> path, sorted by path.
  git -C "$REPO" diff --cached --raw --no-abbrev |
    awk -F'\t' '{ split($1, m, " "); print m[5] "\t" m[4] "\t" $2 }' |
    LC_ALL=C sort -t "$(printf '\t')" -k3 >"$out/$arm.files"
  hash="$(shasum -a 256 <"$out/$arm.diff" | cut -d' ' -f1)"
  count="$(wc -l <"$out/$arm.files" | tr -d ' ')"
  printf '%-11s %8s %9.2f  %s\n' "$arm" "$count" "$(awk -v a="$start" -v b="$end" 'BEGIN { print b - a }')" "$hash" >&2
  [ "$count" -gt 0 ] || die "arm $arm changed no files; the gate would be meaningless"
  if [ -z "$first" ]; then
    first="$arm"
    first_hash="$hash"
  elif [ "$hash" != "$first_hash" ]; then
    status=1
    log "MISMATCH: $arm differs from $first"
    log "differing entries (status, resulting blob, path; '<' $first, '>' $arm):"
    diff "$out/$first.files" "$out/$arm.files" >&2 || true
    log "only in $first: $(LC_ALL=C comm -23 <(cut -f3 "$out/$first.files") <(cut -f3 "$out/$arm.files") | wc -l | tr -d ' ') files;" \
      "only in $arm: $(LC_ALL=C comm -13 <(cut -f3 "$out/$first.files") <(cut -f3 "$out/$arm.files") | wc -l | tr -d ' ') files"
  fi
done

{
  printf 'AVAILABLE_ARMS=%q\n' "$AVAILABLE_ARMS"
  printf 'TS_BRIDGE_FORM=%q\n' "${TS_BRIDGE_FORM:-legacy}"
} >"$RESULTS_DIR/arms.env"

if [ "$status" -ne 0 ]; then
  die "correctness gate FAILED; diffs are in $out"
fi
log "correctness gate passed: $AVAILABLE_ARMS produce identical diffs ($first_hash)"
