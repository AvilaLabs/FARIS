#!/bin/bash
# usage: job.sh <item> <name> <timeout_s> <cores|-> <cmd...>
# One S0c job: memory-capped scope (6 GB, no swap), optional core pinning, /usr/bin/time -v log, wall time and load average
# recorded in the S0c ledger. OMP_NUM_THREADS defaults to 4.
ITEM=$1; NAME=$2; T=$3; CORES=$4; shift 4
B=/home/connoravila/.cache/avila-night/cad-risk-tests/s0c
RT=/home/connoravila/.cache/avila-night/faris-wt/s0c/integrations/openmc/risk_tests
export OMP_NUM_THREADS=${OMP_NUM_THREADS:-4}
PIN=()
[ "$CORES" != "-" ] && PIN=(taskset -c "$CORES")
T0=$(date +%s); L0=$(cat /proc/loadavg)
systemd-run --user --scope --quiet -p MemoryMax=6G -p MemorySwapMax=0 -- timeout --signal=TERM "$T" /usr/bin/time -v "${PIN[@]}" "$@" > "$B/logs/$NAME.log" 2>&1
RC=$?
T1=$(date +%s); L1=$(cat /proc/loadavg)
python3 - "$ITEM" "$NAME" "$((T1-T0))" "$RC" "$L0" "$L1" <<'P'
import sys
sys.path.insert(0, "/home/connoravila/.cache/avila-night/faris-wt/s0c/integrations/openmc/risk_tests")
import s0b_common as c
item, name, wall, rc, l0, l1 = sys.argv[1:7]
d = c.ledger_add("/home/connoravila/.cache/avila-night/cad-risk-tests/s0c/ledger.json", item, name, float(wall), returncode=int(rc), loadavg_start=l0, loadavg_end=l1)
print(f"ledger {name} {wall}s total {d['job_wall_total_s']:.0f}s")
P
exit $RC
