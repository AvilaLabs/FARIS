#!/usr/bin/env bash
# Stage the verified browser bundle and the handbook for `wrangler deploy`.
# Usage: deploy/web/sync.sh VERIFIED_DIST_DIRECTORY
# The directory is the "faris-web" artifact of the Web workflow. Nothing is
# built here: deploys come from the CI artifact, not from a local build.
set -euo pipefail
cd "$(dirname "$0")/../.."
if [[ $# -ne 1 ]]; then
  cat >&2 <<'MSG'
Usage: deploy/web/sync.sh VERIFIED_DIST_DIRECTORY

This script stages a bundle that CI already built and checked; it does not
build one. Download the artifact of the latest successful Web run:

  run_id=$(gh run list --repo AvilaLabs/FARIS --workflow web.yml --branch main \
    --status success --limit 1 --json databaseId --jq '.[0].databaseId')
  gh run download "$run_id" --repo AvilaLabs/FARIS --name faris-web --dir /tmp/faris-web

then run: deploy/web/sync.sh /tmp/faris-web
MSG
  exit 2
fi
build_dist="$(cd "$1" && pwd)"
for asset in index.html faris-app.js faris-app_bg.wasm; do
  [[ -f "$build_dist/$asset" ]] || { echo "Missing browser asset: $asset" >&2; exit 1; }
done
# The destination is cleared completely, so retired files cannot linger.
rm -rf deploy/web/public
mkdir -p deploy/web/public
# Copy everything except a handbook and manifest the artifact may carry; both
# are replaced below, so docs/ is never nested as docs/docs.
(cd "$build_dist" && tar --exclude='./docs' --exclude='./release.json' -cf - .) |
  tar -xf - -C deploy/web/public
if [[ -f "$build_dist/docs/index.html" ]]; then
  cp -R "$build_dist/docs" deploy/web/public/docs
else
  mdbook build
  [[ -f dist/docs/index.html ]] || { echo "The handbook did not build into dist/docs" >&2; exit 1; }
  cp -R dist/docs deploy/web/public/docs
fi
python3 - <<'PY'
import hashlib, json, pathlib, subprocess, tomllib

root = pathlib.Path.cwd()
public = root / "deploy/web/public"
manifest = {
    "schema": "faris-browser-release/v1",
    "version": tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["package"]["version"],
    "source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
    "source_dirty": bool(subprocess.check_output(["git", "status", "--porcelain"], text=True).strip()),
    "assets": {
        path.relative_to(public).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(public.rglob("*")) if path.is_file() and path.name != "release.json"
    },
}
(public / "release.json").write_text(json.dumps(manifest, indent=2) + "\n")
print(f"Staged {len(manifest['assets'])} assets for FARIS {manifest['version']} in {public}")
PY
