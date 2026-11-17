#!/bin/bash
cd /home/user/eb; O=/home/user/eb/results
for g in AUDITCAL SINGLES PAIRS; do
  python3 run.py $g $O/perf-$g.csv > /home/user/logs/perf-$g.out 2>&1 || echo "FAIL $g"
  mkdir -p /mnt/project-files/v2-worker-pool/results && cp $O/*.csv /mnt/project-files/v2-worker-pool/results/
  echo "group $g done"
done
echo ALL-DONE
