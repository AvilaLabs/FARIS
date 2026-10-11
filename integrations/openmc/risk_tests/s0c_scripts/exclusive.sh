#!/bin/bash
# usage: exclusive.sh <name> <cmd...>
# Timing window: snapshots the load average and the busiest processes at start and end into <name>.ps, then runs the command.
# Unlike S0b's wrapper it does not wait for cargo: another builder works on cores 4-7 by arrangement, so the snapshot records it.
NAME=$1; shift
DIR=/home/connoravila/.cache/avila-night/cad-risk-tests/s0c
{ date; cat /proc/loadavg; ps -eo pid,etime,pcpu,args --sort=-pcpu | head -10 | cut -c1-160; } > "$DIR/$NAME.ps"
"$@"
RC=$?
{ echo "--- end"; date; cat /proc/loadavg; ps -eo pid,etime,pcpu,args --sort=-pcpu | head -10 | cut -c1-160; } >> "$DIR/$NAME.ps"
exit $RC
