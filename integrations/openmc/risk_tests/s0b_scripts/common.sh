# sourced by the D2/D3 scripts: PY is set by the caller (the build D1 selected)
B=/home/connoravila/.cache/avila-night/cad-risk-tests/s0b
cd $B
RT=/home/connoravila/.cache/avila-night/faris-wt/s0b/integrations/openmc/risk_tests
M=/home/connoravila/.cache/avila-night/cad-risk-tests/models/rm-m
gen() {   # gen <config> <cores> [extra args]
  local cfg=$1 cores=$2; shift 2
  ./job.sh D3 d3_gen_$cfg 21600 taskset -c $cores $PY $RT/r1_worker.py generate --config $cfg --model-dir $M --run-dir $B/d3/$cfg/generate --out $B/d3/$cfg/generate.json --threads 4 "$@"
}
prod() {  # prod <label> <seed> <windows-config> <cores> [extra args]
  local label=$1 seed=$2 wcfg=$3 cores=$4; shift 4
  while [ ! -f d3/$wcfg/generate/weight_windows.h5 ]; do sleep 15; done
  ./job.sh D3 d3_prod_$label 1900 taskset -c $cores $PY $RT/s0b_run.py run --model-dir $M --run-dir $B/d3/$label/produce --out $B/d3/$label/produce.json --label $label --seed $seed --budget 1200 --windows $B/d3/$wcfg/generate/weight_windows.h5 --particles 10000 --statepoint-every 5 --poll 20 "$@"
}
