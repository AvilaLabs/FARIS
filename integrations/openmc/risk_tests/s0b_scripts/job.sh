#!/bin/bash
# usage: job.sh <item> <logname> <timeout_s> <cmd...>  -- runs under run.sh, records wall time in the S0b ledger
ITEM=$1; NAME=$2; T=$3; shift 3
S=/home/connoravila/.cache/avila-night/faris-wt/s0b/integrations/openmc/risk_tests
T0=$(date +%s); L0=$(cat /proc/loadavg)
/home/connoravila/.cache/avila-night/cad-risk-tests/run.sh "$NAME" "$T" "$@"
RC=$?
T1=$(date +%s); L1=$(cat /proc/loadavg)
python3 - "$ITEM" "$NAME" "$((T1-T0))" "$RC" "$L0" "$L1" <<'P'
import sys
sys.path.insert(0, "/home/connoravila/.cache/avila-night/faris-wt/s0b/integrations/openmc/risk_tests")
import s0b_common as c
item, name, wall, rc, l0, l1 = sys.argv[1:7]
c.ledger_add("/home/connoravila/.cache/avila-night/cad-risk-tests/s0b/ledger.json", item, name, float(wall), returncode=int(rc), loadavg_start=l0, loadavg_end=l1)
P
exit $RC
