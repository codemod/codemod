#!/usr/bin/env bash
# Rebuilds depq and reruns every result in results/.
# Usage: ./demo.sh <workdir> <codemod checkout> [<other repo> <eco:name@version>]
# Needs cargo, git, curl, and network to github.com, registry.npmjs.org and index.crates.io.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="${1:?workdir}"; CODEMOD="${2:?codemod checkout}"; OTHER="${3:-}"; OTHER_TARGET="${4:-}"
COMMIT=39f85f35e2497d989133cbf20da8445a31b39222   # the Backstage commit the case study used
export CARGO_TARGET_DIR="$WORK/target"
(cd "$HERE/depq" && cargo build --release -q && cargo test --release -q)
D="$CARGO_TARGET_DIR/release/depq"; R="$HERE/results"; Q="$HERE/queries"; BS="$WORK/backstage"
if [ ! -d "$BS/.git" ]; then
  git init -q "$BS" && git -C "$BS" fetch -q --depth 1 https://github.com/backstage/backstage "$COMMIT" && git -C "$BS" checkout -q FETCH_HEAD
fi
IGN='react,react-dom,react-test-renderer,react-reconciler,@yarnpkg/*,ink-*'

echo "== A. pattern queries (Backstage, yarn)"
$D "$BS" query 'ws:* ~> npm:react-beautiful-dnd'
$D "$BS" query 'ws:* ~> npm:@material-ui/core' | grep -c '"from"'
$D "$BS" stuck 'ext:* -[peer]-> npm:react refuses 19.0.0' > "$R/backstage-stuck.json"
echo "== A. Cargo (codemod)"
$D "$CODEMOD" query 'ws:* ~> cargo:bitflags@^1' > "$R/codemod-bitflags1-paths.json"
echo "== B. SQL"
for q in reach refuses bump-plan; do $D "$BS" sql "$Q/$q.sql" > "$R/backstage-sql-$q.tsv"; done
$D "$CODEMOD" sql "$Q/cargo-dupes.sql" > "$R/codemod-sql-cargo-dupes.tsv"
echo "== C. impact (graph joined with code)"
$D "$BS" impact npm:react@19.0.0 --also react-dom --ignore "$IGN" > "$R/backstage-react19-impact.json"
$D "$CODEMOD" impact cargo:tree-sitter@0.27.2 > "$R/codemod-tree-sitter-0.27-impact.json"
[ -n "$OTHER" ] && $D "$OTHER" impact "$OTHER_TARGET" > "$WORK/other-impact.json" && echo "other repo: $WORK/other-impact.json"
python3 -c 'import json,sys; [print(f, json.load(open(f))["summary"]) for f in sys.argv[1:]]' "$R"/*-impact.json
