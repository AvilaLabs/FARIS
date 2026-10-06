#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Development review launch: open an unpacked 0.1.1+ package (app and evidence parts)
# in package mode. FARIS_APP names a locally built faris-app instead of the package's
# own; the app then labels itself a development binary not covered by the package index.
#   FARIS_PACKAGE=/path/to/FARIS-0.1.1 [FARIS_APP=/path/to/faris-app] scripts/launch_review_demo.sh [app arguments...]
set -eu
package=${FARIS_PACKAGE:?set FARIS_PACKAGE to an unpacked package folder (0.1.1 or later)}
app=${FARIS_APP:-$package/bin/faris-app}
exec "$app" --package "$package" "$@"
