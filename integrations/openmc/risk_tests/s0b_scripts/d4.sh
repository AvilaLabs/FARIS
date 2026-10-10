#!/bin/bash
# D4 (alone, 8 threads): the 60 s preview twice with G*'s windows (G6 + max_split 1000 + checkpoints), then the start-up split
cd /home/connoravila/.cache/avila-night/cad-risk-tests/s0b
PY=/home/connoravila/micromamba/envs/openmc0153dd/bin/python
RT=/home/connoravila/.cache/avila-night/faris-wt/s0b/integrations/openmc/risk_tests
M=/home/connoravila/.cache/avila-night/cad-risk-tests/models/rm-m
B=$PWD
W=$B/d3/G6/generate/weight_windows.h5
mkdir -p d4
for N in 1 2; do
  OMP_NUM_THREADS=8 ./job.sh D4 d4_preview_$N 300 $PY $RT/r2_preview.py --model-dir $M --windows $W --run-dir $B/d4/run$N --out $B/d4/run$N.json --threads 8 --budget 60 --seed 90060001 --max-split 1000 --checkpoints
done
# start-up split: one short batch with and without the windows (the log's initialization and cross-section times)
OMP_NUM_THREADS=8 ./job.sh D4 d4_startup_nowindows 600 $PY $RT/s0b_run.py run --model-dir $M --run-dir $B/d4/startup_nowin --out $B/d4/startup_nowin.json --label D4-startup-nowindows --seed 90060001 --histories 2000 --particles 1000 --statepoint-every 0 --threads 8
OMP_NUM_THREADS=8 ./job.sh D4 d4_startup_windows 600 $PY $RT/s0b_run.py run --model-dir $M --run-dir $B/d4/startup_win --out $B/d4/startup_win.json --label D4-startup-windows --seed 90060001 --histories 2000 --particles 1000 --statepoint-every 0 --threads 8 --windows $W --max-split 1000 --checkpoints
