#!/usr/bin/env bash
# Clone the pinned vitejs/vite release into a fresh, empty directory outside
# this repository: `setup.sh [directory]` (default: $BENCH_REPO, else
# $HOME/.cache/codemod-bench/vite).
#
# The clone is untrusted data. This script only runs git against it (by path,
# with hooks disabled and no submodules, LFS, or checkout filters); it never
# runs the clone's scripts, never installs its dependencies, and never
# executes tooling from inside it.
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"

if [ $# -gt 0 ]; then REPO="$1"; fi
case "$REPO" in /*) ;; *) REPO="$PWD/$REPO" ;; esac
case "$REPO/" in "$ROOT"/*) die "the checkout must live outside this repository: $REPO" ;; esac

# Already set up: just make it pristine.
if [ -d "$REPO/.git" ] && [ "$(git -C "$REPO" rev-parse HEAD 2>/dev/null)" = "$VITE_SHA" ]; then
  reset_repo "$REPO"
  log "vite $VITE_TAG ($VITE_SHA) already at $REPO; reset and cleaned"
  exit 0
fi

if [ -e "$REPO" ] && [ -n "$(ls -A "$REPO" 2>/dev/null)" ]; then
  die "$REPO exists and is not empty; remove it or choose another directory (BENCH_REPO=...)"
fi
mkdir -p "$REPO"

# Defensive git: no hooks, no filters or LFS smudging, no submodules, no
# file:// or ext:: transports.
safe_git() {
  git -C "$REPO" \
    -c core.hooksPath=/dev/null \
    -c core.fsmonitor=false \
    -c filter.lfs.smudge= -c filter.lfs.process= -c filter.lfs.required=false \
    -c protocol.file.allow=never -c protocol.ext.allow=never \
    -c submodule.recurse=false \
    -c advice.detachedHead=false \
    "$@"
}

log "cloning $VITE_URL $VITE_TAG into $REPO"
git init -q "$REPO"
# Persist the defensive settings for later resets (run.sh/verify.sh).
git -C "$REPO" config core.hooksPath /dev/null
git -C "$REPO" config core.fsmonitor false
safe_git fetch -q --depth 1 --no-tags "$VITE_URL" "refs/tags/$VITE_TAG"
fetched="$(safe_git rev-parse 'FETCH_HEAD^{commit}')"
[ "$fetched" = "$VITE_SHA" ] ||
  die "tag $VITE_TAG resolved to $fetched, expected $VITE_SHA; refusing to continue"
safe_git checkout -q --detach "$VITE_SHA"
[ -z "$(git -C "$REPO" status --porcelain)" ] || die "fresh checkout is not clean"

log "vite $VITE_TAG at $VITE_SHA: $(git -C "$REPO" ls-files | wc -l | tr -d ' ') tracked files"
log "next: $(dirname "$0")/verify.sh   (BENCH_REPO=$REPO)"
