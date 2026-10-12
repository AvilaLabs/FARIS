#!/usr/bin/env bash
# S0f production (docs/notes/CAD_TRANSPORT_S0F.md, "Protocol"): F0 then F1 for each seed, interleaved.
# Each run waits up to 10 min for a 1-minute load below 3.0 (waiting is not job time), then runs on cores 0-3 under a
# 6 GB cap. Jobs stop when the next run would push job time past 75 min; runs not started are NOT_RUN.
set -u
RT=$(cd "$(dirname "$0")/.." && pwd)
OUT=${OUT:-$HOME/.cache/avila-night/cad-risk-tests/s0f}
BASE=$OUT/base-model.xml
PY=$HOME/micromamba/envs/openmc0153dd/bin/python
SHADOW=$HOME/.cache/avila-night/openmc-presplit/pyshadow
APERTURE=$($PY -c "import sys; sys.path.insert(0, '$RT'); import s0f_common as F; print(F.APERTURE)")
SEEDS=$($PY -c "import sys; sys.path.insert(0, '$RT'); import s0f_common as F; print(' '.join(map(str, F.SEEDS)))")
CAP_S=4500
EST_S=480
used=0
log() { echo "$(date -Is) $*" | tee -a "$OUT/run.log"; }
for seed in $SEEDS; do
  for cfg in F0 F1; do
    if [ $((used + EST_S)) -gt $CAP_S ]; then log "$cfg-$seed NOT_RUN (time box)"; continue; fi
    waited=0
    while :; do
      l=$(cut -d' ' -f1 /proc/loadavg)
      if awk "BEGIN{exit !($l < 3.0)}" || [ $waited -ge 600 ]; then break; fi
      sleep 30; waited=$((waited + 30))
    done
    log "$cfg-$seed start (load $l, waited ${waited}s)"
    t0=$(date +%s)
    if [ $cfg = F0 ]; then
      systemd-run --user --scope -q -p MemoryMax=6G -p MemorySwapMax=0 \
        env -u OPENMC_WW_SKIP_VOID -u OPENMC_APERTURE_BIAS -u OPENMC_SPLIT_BEFORE_COLLISION -u PYTHONPATH \
        systemd-inhibit --what=sleep:idle --who=FARIS --why=S0f taskset -c 0-3 \
        $PY "$RT/s0f_scan.py" --base-model "$BASE" --run-dir "$OUT/$cfg-$seed" --config $cfg --seed $seed \
        > "$OUT/$cfg-$seed.log" 2>&1
    else
      systemd-run --user --scope -q -p MemoryMax=6G -p MemorySwapMax=0 \
        env -u OPENMC_SPLIT_BEFORE_COLLISION PYTHONPATH="$SHADOW" OPENMC_WW_SKIP_VOID=1 "OPENMC_APERTURE_BIAS=$APERTURE" \
        systemd-inhibit --what=sleep:idle --who=FARIS --why=S0f taskset -c 0-3 \
        $PY "$RT/s0f_scan.py" --base-model "$BASE" --run-dir "$OUT/$cfg-$seed" --config $cfg --seed $seed \
        > "$OUT/$cfg-$seed.log" 2>&1
    fi
    rc=$?
    used=$((used + $(date +%s) - t0))
    log "$cfg-$seed exit $rc, job time so far ${used}s: $(tail -1 "$OUT/$cfg-$seed.log")"
  done
done
$PY "$RT/s0f_analyse.py" --runs "$OUT" --out "$OUT/results-s0f.json" | tee -a "$OUT/run.log"
