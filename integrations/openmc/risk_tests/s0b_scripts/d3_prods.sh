#!/bin/bash
# cores 0-3: the G0, G3, G4, G2 productions (G1 is NOT_EVALUATED: generation killed by the 6G cap)
export PY=/home/connoravila/micromamba/envs/openmc0153dd/bin/python
source /home/connoravila/.cache/avila-night/cad-risk-tests/s0b/common.sh
prod G0 90040000 G0 0-3
prod G3 90040003 G0 0-3 --max-split 1000
prod G4 90040004 G0 0-3 --checkpoints
prod G2 90040002 G2 0-3
