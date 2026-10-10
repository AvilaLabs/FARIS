#!/bin/bash
# D5a: the D1S run of every material x source repeated on OpenMC 0.16.0 (cores 4-7)
cd /home/connoravila/.cache/avila-night/cad-risk-tests/s0b
S=/home/connoravila/.cache/avila-night/faris-wt/s0b/integrations/openmc/risk_tests/s0b_d5a.py
PY=/home/connoravila/micromamba/envs/openmc016/bin/python
export PATH=/home/connoravila/micromamba/envs/openmc016/bin:$PATH
for m in tungsten iron_surrogate tih2; do
  ./job.sh D5a d5a_d1s_${m}_mono14_v016 3600 taskset -c 4-7 $PY $S d1s --material $m --source mono14 --out-dir d5a/${m}_mono14_v016 --batches 50 --particles 200000
  ./job.sh D5a d5a_d1s_${m}_tf_v016 3600 taskset -c 4-7 $PY $S d1s --material $m --source tf --spectrum d5a/tf_spectrum.json --out-dir d5a/${m}_tf_v016 --batches 50 --particles 200000
done
