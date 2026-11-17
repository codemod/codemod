#!/usr/bin/env bash
# Cost of the TS build step: run each TS workflow on the 2-file probe repo with
# an empty build cache, then with a warm one. 3 rounds each. Prints CSV.
set -u
cd "$(dirname "$0")"
export CODEMOD_QUICKJS_HOST_BIN=/home/user/ts-orch/target/release/orchestration-quickjs-host
export CODEMOD_JSSG_NAPI=/home/user/ts-orch/packages/jssg-napi/jssg-napi.linux-x64.node
export NO_COLOR=1
CACHE=$(mktemp -d); export CODEMOD_WORKFLOW_CACHE=$CACHE
REPO=/home/user/bench-data/repos/probe
echo "workflow,runtime,cache,round,wall_s"
for wf in util-is react-19 debarrel audit shell-chain; do
  for rt in node quickjs; do
    for cache in cold warm; do
      for r in 1 2 3; do
        [ $cache = cold ] && rm -rf "$CACHE"/*
        [ $cache = warm ] && [ $r = 1 ] && (cd /tmp && node /home/user/ts-orch/packages/orchestration/bin/codemod-workflow.mjs run /home/user/bench/workflows/$wf/ts/workflow.ts --target $REPO --runtime=$rt >/dev/null 2>&1); git -C $REPO reset -q --hard; git -C $REPO clean -qfdx
        s=$(date +%s.%N)
        (cd /tmp && node /home/user/ts-orch/packages/orchestration/bin/codemod-workflow.mjs run /home/user/bench/workflows/$wf/ts/workflow.ts --target $REPO --runtime=$rt >/dev/null 2>&1)
        e=$(date +%s.%N)
        git -C $REPO reset -q --hard; git -C $REPO clean -qfdx
        echo "$wf,$rt,$cache,$r,$(python3 -c "print(round($e-$s,3))")"
      done
    done
  done
done
rm -rf "$CACHE"
