#!/bin/bash
# D5a for the 14.1 MeV source: all three materials, pinned to cores 4-7
cd /home/connoravila/.cache/avila-night/cad-risk-tests/s0b
S=/home/connoravila/.cache/avila-night/faris-wt/s0b/integrations/openmc/risk_tests/s0b_d5a.py
PY=/home/connoravila/micromamba/envs/openmc0153dd/bin/python
for m in tungsten iron_surrogate tih2; do
  ./job.sh D5a d5a_photon_$m 3600 taskset -c 4-7 $PY $S photon --material $m --out-dir d5a/$m --workers 4
  ./job.sh D5a d5a_neutron_${m}_mono14 3600 taskset -c 4-7 $PY $S neutron --material $m --source mono14 --out-dir d5a/${m}_mono14 --batches 50 --particles 200000
  ./job.sh D5a d5a_d1s_${m}_mono14 3600 taskset -c 4-7 $PY $S d1s --material $m --source mono14 --out-dir d5a/${m}_mono14 --batches 50 --particles 200000
  ./job.sh D5a d5a_photon_radial_${m}_mono14 3600 taskset -c 4-7 $PY $S photon --material $m --out-dir d5a/$m --workers 4 --radial-from d5a/${m}_mono14/rates.json
  ./job.sh D5a d5a_analyse_${m}_mono14 600 taskset -c 4-7 $PY $S analyse --material $m --source mono14 --out-dir d5a/${m}_mono14 --photon-dir d5a/$m
done
