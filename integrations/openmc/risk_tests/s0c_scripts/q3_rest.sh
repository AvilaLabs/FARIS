#!/bin/bash
# Q3 R2S runs after the 10 cm mesh (which used 20 x 100k neutrons and 20 x 100k photons): 2 cm mesh, 20 cm mesh, 2 cm cells.
B=/home/connoravila/.cache/avila-night/cad-risk-tests/s0c
RT=/home/connoravila/.cache/avila-night/faris-wt/s0c/integrations/openmc/risk_tests
PY=/home/connoravila/micromamba/envs/openmc0153dd/bin/python
J=$RT/s0c_scripts/job.sh
SIZES="--neutron-batches 10 --neutron-particles 50000 --photon-batches 20 --photon-particles 300000"
$J Q3 q3_mesh2 400 0-3 $PY $RT/s0c_p3.py r2s --mode mesh --voxel 2 --out-dir $B/q3/mesh2 $SIZES
$J Q3 q3_mesh20 400 0-3 $PY $RT/s0c_p3.py r2s --mode mesh --voxel 20 --out-dir $B/q3/mesh20 $SIZES
$J Q3 q3_cell2 400 0-3 $PY $RT/s0c_p3.py r2s --mode cell --out-dir $B/q3/cell2 $SIZES
