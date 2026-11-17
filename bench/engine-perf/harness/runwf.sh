#!/bin/bash
# runwf.sh <bin-name> <workflow> <repo>: one run of a YAML workflow via one.py
E=/home/user/eb; B=$1; [ "$B" = mo ] && B=mo/codemod
python3 $E/one.py $E/data/repos/$3 $E/bin/$B workflow run -w $E/bench/workflows/$2/yaml/workflow.yaml --no-interactive --disable-analytics --allow-fs
