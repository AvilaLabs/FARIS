#!/bin/bash
# D1: the R6 analog model, 1e6 histories, 4 threads, same new seed, on both builds, one after the other
cd /home/connoravila/.cache/avila-night/cad-risk-tests/s0b
S=/home/connoravila/.cache/avila-night/faris-wt/s0b/integrations/openmc/risk_tests/s0b_run.py
M=/home/connoravila/.cache/avila-night/cad-risk-tests/models/rm-m
mkdir -p d1
./job.sh D1 d1_dd 2400 /home/connoravila/micromamba/envs/openmc0153dd/bin/python $S run --model-dir $M --run-dir d1/dd_run --out d1/dd.json --label D1-openmc0153dd --seed 90010001 --histories 1000000 --particles 10000 --statepoint-every 0 --threads 4
./job.sh D1 d1_moab 7200 /home/connoravila/.venvs/w003env/bin/python $S run --model-dir $M --run-dir d1/moab_run --out d1/moab.json --label D1-w003env --seed 90010001 --histories 1000000 --particles 10000 --statepoint-every 0 --threads 4
