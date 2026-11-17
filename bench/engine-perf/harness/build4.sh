#!/bin/bash
cd /home/user/wt/mo; export CARGO_TARGET_DIR=/home/user/tgt/mo
for b in kinds rules cap stack5; do
  git checkout -q perf/$b || { echo "FAIL checkout $b"; continue; }
  if cargo build --release -p codemod > /home/user/logs/build-$b.log 2>&1; then cp $CARGO_TARGET_DIR/release/codemod /home/user/eb/bin/mo-$b-pure; echo "built $b"; else echo "FAIL $b"; fi
done
echo BUILDS-DONE
