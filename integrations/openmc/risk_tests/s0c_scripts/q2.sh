#!/bin/bash
# Q2 (alone, 8 threads): cold start breakdown, then two warm previews of 60 s with the same seed, in one openmc.lib process.
B=/home/connoravila/.cache/avila-night/cad-risk-tests/s0c
RT=/home/connoravila/.cache/avila-night/faris-wt/s0c/integrations/openmc/risk_tests
PY=/home/connoravila/micromamba/envs/openmc0153dd/bin/python
OMP_NUM_THREADS=8 $RT/s0c_scripts/exclusive.sh q2 $RT/s0c_scripts/job.sh Q2 q2_run 600 - $PY $RT/s0c_q2.py run --run-dir $B/q2/run --out $B/q2/q2.json --threads 8 --previews 2 --budget 60
