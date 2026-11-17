#!/bin/bash
cd /home/user/eb
O=/mnt/project-files/v2-worker-pool/results
python3 run.py PERF $O/perf-main.csv > /home/user/logs/perf-main.out 2>&1
python3 run.py PAIRS $O/perf-pairs.csv > /home/user/logs/perf-pairs.out 2>&1
echo DONE >> /home/user/logs/perf-pairs.out
