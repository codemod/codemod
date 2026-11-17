#!/usr/bin/env bash
# Recreates the bench inputs: pinned codemod sources, pinned target repos, the
# probe repo, the *.port.ts copies the TS arms use, and the symlinks the
# workflow folders expect. Paths match config.json; edit both if you move them.
set -euo pipefail
BENCH=${BENCH:-/home/user/bench}          # where this folder's contents live
DATA=${DATA:-/home/user/bench-data}
TS_ORCH=${TS_ORCH:-/home/user/ts-orch}    # prototype/typescript-orchestration + patches/ts-orch
mkdir -p "$DATA/src" "$DATA/repos"

clone() { # url dir sha
  [ -d "$2/.git" ] || git clone --quiet "$1" "$2"
  git -C "$2" fetch --quiet --depth 1 origin "$3" 2>/dev/null || true
  git -C "$2" checkout --quiet "$3"
}
clone https://github.com/codemod/react-codemod        "$DATA/src/codemod__react-codemod"        8936057c2a018ad4833a8f95d72b0154092ad19e
clone https://github.com/codemod/useful-codemods      "$DATA/src/codemod__useful-codemods"      c655976690f86404190d7e54af3154881f39d76a
clone https://github.com/nodejs/userland-migrations   "$DATA/src/nodejs__userland-migrations"   8beab324723bfe2c31c81981a6ffabe96845e498
clone https://github.com/shadcn-ui/taxonomy           "$DATA/repos/shadcn-ui__taxonomy"         298a8857c7128a0d121e7f699dfd729f23b3966d
clone https://github.com/excalidraw/excalidraw        "$DATA/repos/excalidraw__excalidraw"      701b901a0f1ba3d0b390d9cdf63d112e25abac4b
clone https://github.com/calcom/cal.com               "$DATA/repos/calcom__cal.com"             2eb48eccd8aca8a0c0ffabf28cf92e298b33ecee
clone https://github.com/petkaantonov/bluebird        "$DATA/repos/petkaantonov__bluebird"      c220cfe480868e2f5c5de4fe8bb3b04b97588d7f
clone https://github.com/react-bootstrap/react-bootstrap "$DATA/repos/react-bootstrap-v1"       b415c70022291a1aa8751d7083beabf3dbbbb59d  # tag v1.6.8

# Probe repo: one util.isArray call and one ReactDOM.render call.
P="$DATA/repos/probe"
if [ ! -d "$P/.git" ]; then
  mkdir -p "$P/src" && git -C "$P" init --quiet
  printf 'import util from "node:util";\nexport const isList = (x: unknown) => util.isArray(x);\n' > "$P/src/a.ts"
  printf 'import ReactDOM from "react-dom";\nimport { App } from "./app";\nReactDOM.render(<App />, document.getElementById("root"));\n' > "$P/src/App.tsx"
  git -C "$P" add -A && git -C "$P" -c user.name=bench -c user.email=bench@example.com commit --quiet -m probe
fi

# Codemod runtime deps (production only, no install scripts).
for d in "$DATA/src/codemod__react-codemod" "$DATA/src/codemod__react-codemod/codemods/"{replace-use-form-state,replace-reactdom-render,replace-string-ref,replace-act-import,prop-types-typescript} \
         "$DATA/src/nodejs__userland-migrations" "$DATA/src/codemod__useful-codemods" "$DATA/src/codemod__useful-codemods/codemods/debarrel"; do
  (cd "$d" && npm install --omit=dev --ignore-scripts --no-audit --no-fund --silent)
done

# TS arms: neither TS runtime serves codemod:metrics, so the TS arms import a shim.
SHIM="$BENCH/workflows/_shared/metrics-shim.ts"
for f in "$DATA/src/codemod__react-codemod/codemods/"{replace-reactdom-render,replace-string-ref,replace-act-import,replace-use-form-state,prop-types-typescript}/scripts/codemod.ts \
         "$DATA/src/codemod__useful-codemods/codemods/debarrel/scripts/codemod.ts"; do
  sed "s#from \"codemod:metrics\"#from \"$SHIM\"#" "$f" > "${f%.ts}.port.ts"
done

W="$BENCH/workflows"
ln -sfn "$DATA/src/codemod__react-codemod/codemods"                         "$W/react-19/pkgs"
ln -sfn "$DATA/src/codemod__useful-codemods/codemods/debarrel"              "$W/debarrel/pkg"
ln -sfn "$DATA/src/codemod__useful-codemods/codemods/debarrel/scripts"      "$W/debarrel/yaml/scripts"
ln -sfn "$DATA/src/nodejs__userland-migrations/recipes/util-is"             "$W/util-is/pkg"
ln -sfn "$DATA/src/nodejs__userland-migrations/recipes/util-is/src"         "$W/util-is/yaml/src"
mkdir -p "$W/node_modules/@codemod.com"
ln -sfn "$TS_ORCH/packages/orchestration"                                   "$W/node_modules/@codemod.com/orchestration"
echo "inputs ready"
