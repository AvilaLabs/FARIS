#!/bin/bash
# cores 4-7: generations G0, G1, G2 one after the other
export PY=/home/connoravila/micromamba/envs/openmc0153dd/bin/python
source /home/connoravila/.cache/avila-night/cad-risk-tests/s0b/common.sh
mkdir -p d3
gen G0 4-7
gen G1 4-7
gen G2 4-7
