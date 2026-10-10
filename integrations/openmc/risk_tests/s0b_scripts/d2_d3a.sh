#!/bin/bash
# cores 0-3: D2 analog reference, then the five G0..G4 productions (windows from G0 for G3/G4)
export PY=/home/connoravila/micromamba/envs/openmc0153dd/bin/python
source /home/connoravila/.cache/avila-night/cad-risk-tests/s0b/common.sh
RTD=$RT
mkdir -p d2
./job.sh D2 d2_analog 7800 taskset -c 0-3 $PY $RT/s0b_run.py run --model-dir $M --run-dir $B/d2/run --out $B/d2/analog.json --label D2-analog --seed 90020001 --particles 10000 --budget 7200 --stop-r 0.05 --statepoint-every 25 --poll 30
prod G0 90040000 G0 0-3
prod G3 90040003 G0 0-3 --max-split 1000
prod G4 90040004 G0 0-3 --checkpoints
prod G1 90040001 G1 0-3
prod G2 90040002 G2 0-3
