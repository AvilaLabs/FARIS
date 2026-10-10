#!/bin/bash
# usage: exclusive.sh <name> <cmd...>
# Timing window alone: waits until no cargo/rustc runs, creates the S0B_EXCLUSIVE marker (removed by trap), snapshots the busiest
# processes at start and end into <name>.ps, then runs the command (which starts its jobs through job.sh).
NAME=$1; shift
MARK=/home/connoravila/.cache/avila-night/cad-risk-tests/S0B_EXCLUSIVE
DIR=/home/connoravila/.cache/avila-night/cad-risk-tests/s0b
while ps -eo args | grep -q "[c]argo\|[r]ustc"; do sleep 20; done
trap 'rm -f "$MARK"' EXIT INT TERM
touch "$MARK"
{ date; cat /proc/loadavg; ps -eo pid,etime,pcpu,args --sort=-pcpu | head -8 | cut -c1-160; } > "$DIR/$NAME.ps"
"$@"
RC=$?
{ echo "--- end"; date; cat /proc/loadavg; ps -eo pid,etime,pcpu,args --sort=-pcpu | head -8 | cut -c1-160; } >> "$DIR/$NAME.ps"
exit $RC
