#!/bin/bash
# Q1 resolution sweep: for each spacing, windows by random ray then a 120 s production, all on cores 0-3 at 4 threads.
B=/home/connoravila/.cache/avila-night/cad-risk-tests/s0c
RT=/home/connoravila/.cache/avila-night/faris-wt/s0c/integrations/openmc/risk_tests
PY=/home/connoravila/micromamba/envs/openmc0153dd/bin/python
J=$RT/s0c_scripts/job.sh
cd $B
for D in "$@"; do
  $J Q1 q1_gen_$D 900 0-3 $PY $RT/s0c_p1.py generate --spacing $D --mgxs $B/q1/mgxs/mgxs.h5 --run-dir $B/q1/gen_$D --out $B/q1/gen_$D.json
  $J Q1 q1_prod_$D 400 0-3 $PY $RT/s0c_p1.py produce --label sp$D --windows $B/q1/gen_$D/weight_windows.h5 --run-dir $B/q1/prod_$D --out $B/q1/prod_$D.json --seed $($PY -c "import sys;sys.path.insert(0,'$RT');import s0c_common as S;print(S.SEEDS['prod'][$D])")
done
