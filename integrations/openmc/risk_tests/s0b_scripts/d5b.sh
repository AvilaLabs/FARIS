#!/bin/bash
# D5b: where D1S's time goes: four RM-M configurations, 300 s each, 4 threads on cores 0-3 (nothing else of ours running)
cd /home/connoravila/.cache/avila-night/cad-risk-tests/s0b
PY=/home/connoravila/micromamba/envs/openmc0153dd/bin/python
RT=/home/connoravila/.cache/avila-night/faris-wt/s0b/integrations/openmc/risk_tests
M=/home/connoravila/.cache/avila-night/cad-risk-tests/models/rm-m
B=$PWD
mkdir -p d5b
for N in 1 2 3 4; do
  ./job.sh D5b d5b_mode$N 1500 taskset -c 0-3 $PY $RT/s0b_d5b.py --mode $N --model-dir $M --run-dir $B/d5b/mode${N}_run --out $B/d5b/mode$N.json --budget 300
done
