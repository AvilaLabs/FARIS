#!/bin/bash
# D5a remainder, cores 4-7: iron_surrogate mono14 after its D1S run (already running), photon radial and analysis
cd /home/connoravila/.cache/avila-night/cad-risk-tests/s0b
S=/home/connoravila/.cache/avila-night/faris-wt/s0b/integrations/openmc/risk_tests/s0b_d5a.py
PY=/home/connoravila/micromamba/envs/openmc0153dd/bin/python
m=iron_surrogate
while [ ! -f d5a/${m}_mono14/d1s.json ]; do sleep 10; done
./job.sh D5a d5a_photon_radial_${m}_mono14 3600 taskset -c 4-7 $PY $S photon --material $m --out-dir d5a/$m --workers 4 --radial-from d5a/${m}_mono14/rates.json
./job.sh D5a d5a_analyse_${m}_mono14 600 taskset -c 4-7 $PY $S analyse --material $m --source mono14 --out-dir d5a/${m}_mono14 --photon-dir d5a/$m
