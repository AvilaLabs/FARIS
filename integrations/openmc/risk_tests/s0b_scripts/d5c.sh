#!/bin/bash
# D5c: S0's R2S neutron step + activation with two new seeds, photon step at 1 d only, on the D1 build (cores 4-7)
cd /home/connoravila/.cache/avila-night/cad-risk-tests/s0b
PY=/home/connoravila/micromamba/envs/openmc0153dd/bin/python
RT=/home/connoravila/.cache/avila-night/faris-wt/s0b/integrations/openmc/risk_tests
M=/home/connoravila/.cache/avila-night/cad-risk-tests/models/rm-m
for S in 90080001 90080011; do
  ./job.sh D5c d5c_$S 14400 taskset -c 4-7 $PY $RT/r3_r2s.py --model-dir $M --out-dir /home/connoravila/.cache/avila-night/cad-risk-tests/s0b/d5c/$S --neutron-particles 20000 --neutron-batches 40 --photon-particles 20000 --photon-max-batches 180 --photon-budget 3000 --photon-indices 2 --seed $S
done
