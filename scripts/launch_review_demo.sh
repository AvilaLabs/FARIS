#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Development review launch: the recorded four-case package, the allocation
# sweep bundles, and a locally built faris-app (not covered by the package index).
#   FARIS_APP=/path/to/faris-app scripts/launch_review_demo.sh [app arguments...]
set -eu
export PYTHONDONTWRITEBYTECODE=1
repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
package=${FARIS_PACKAGE:-$repo/dist/FARIS-demo-2026-10-01}
app=${FARIS_APP:-$repo/target/release/faris-app}
sweep=${FARIS_SWEEP_BUNDLES:-$repo/runs/allocation-sweep/bundles}
set --  "$@"
for bundle in "$sweep"/blanket-*.transport-bundle.json; do
  [ -f "$bundle" ] && set -- "$@" --sweep-bundle "$bundle"
done
python3 "$package/scripts/verify_binary_manifest.py" "$package"
exec python3 "$repo/scripts/launch_recorded_demo.py" "$package" --app "$app" "$@"
