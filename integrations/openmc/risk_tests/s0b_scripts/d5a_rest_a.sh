#!/bin/bash
# D5a remainder, cores 0-3: tih2 (photon reference, then the mono14 neutron and D1S runs)
cd /home/connoravila/.cache/avila-night/cad-risk-tests/s0b
S=/home/connoravila/.cache/avila-night/faris-wt/s0b/integrations/openmc/risk_tests/s0b_d5a.py
PY=/home/connoravila/micromamba/envs/openmc0153dd/bin/python
m=tih2
./job.sh D5a d5a_photon_$m 3600 taskset -c 0-3 $PY $S photon --material $m --out-dir d5a/$m --workers 4
./job.sh D5a d5a_neutron_${m}_mono14 3600 taskset -c 0-3 $PY $S neutron --material $m --source mono14 --out-dir d5a/${m}_mono14 --batches 50 --particles 200000
./job.sh D5a d5a_d1s_${m}_mono14 3600 taskset -c 0-3 $PY $S d1s --material $m --source mono14 --out-dir d5a/${m}_mono14 --batches 50 --particles 200000
./job.sh D5a d5a_photon_radial_${m}_mono14 3600 taskset -c 0-3 $PY $S photon --material $m --out-dir d5a/$m --workers 4 --radial-from d5a/${m}_mono14/rates.json
./job.sh D5a d5a_analyse_${m}_mono14 600 taskset -c 0-3 $PY $S analyse --material $m --source mono14 --out-dir d5a/${m}_mono14 --photon-dir d5a/$m
