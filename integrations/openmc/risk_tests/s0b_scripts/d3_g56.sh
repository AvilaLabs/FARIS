#!/bin/bash
# cores 0-3: G5 (= G0 windows + max_split 1000 + checkpoints: neither G1 nor G2 improved), then G6 (9 groups, regenerated) with the same flags
export PY=/home/connoravila/micromamba/envs/openmc0153dd/bin/python
source /home/connoravila/.cache/avila-night/cad-risk-tests/s0b/common.sh
prod G5 90040005 G0 0-3 --max-split 1000 --checkpoints
gen G6 0-3
prod G6 90040006 G6 0-3 --max-split 1000 --checkpoints
