#!/bin/bash
# D5a for the TF fast-flux spectrum source, pinned to cores 0-3 (waits for each material's photon reference from the mono14 chain)
cd /home/connoravila/.cache/avila-night/cad-risk-tests/s0b
S=/home/connoravila/.cache/avila-night/faris-wt/s0b/integrations/openmc/risk_tests/s0b_d5a.py
PY=/home/connoravila/micromamba/envs/openmc0153dd/bin/python
for m in tungsten iron_surrogate tih2; do
  ./job.sh D5a d5a_neutron_${m}_tf 3600 taskset -c 0-3 $PY $S neutron --material $m --source tf --spectrum d5a/tf_spectrum.json --out-dir d5a/${m}_tf --batches 50 --particles 200000
  ./job.sh D5a d5a_d1s_${m}_tf 3600 taskset -c 0-3 $PY $S d1s --material $m --source tf --spectrum d5a/tf_spectrum.json --out-dir d5a/${m}_tf --batches 50 --particles 200000
  while [ ! -f d5a/$m/photon_ref.json ]; do sleep 10; done
  ./job.sh D5a d5a_photon_radial_${m}_tf 3600 taskset -c 0-3 $PY $S photon --material $m --out-dir d5a/$m --workers 4 --radial-from d5a/${m}_tf/rates.json
  ./job.sh D5a d5a_analyse_${m}_tf 600 taskset -c 0-3 $PY $S analyse --material $m --source tf --out-dir d5a/${m}_tf --photon-dir d5a/$m
done
