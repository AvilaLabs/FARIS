#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
# Run every fuzz target for the same number of seconds and write the evidence file the release
# gate reads (QA-033, scripts/release_gate.py --fuzz).
#
#   scripts/fuzz_all.sh SECONDS_PER_TARGET [EVIDENCE_FILE] [TARGET]
#
# The release run is 3600 seconds per target (QA-033: at least 1 CPU-hour each, 19 hours in all);
# a quick check is 30 or 60. Every target runs under libFuzzer's own limits (-rss_limit_mb=2048,
# -timeout=10; no parallel jobs) inside a 3 GB memory cgroup and two CPUs (taskset), so a runaway
# input cannot take the machine down.
#
# DRIVER=libfuzzer (default) builds with `cargo +nightly fuzz` and needs the nightly toolchain,
# cargo-fuzz and the libfuzzer-sys crate. DRIVER=smoke runs the blind mutation driver in
# fuzz/entries instead, for a machine that cannot fetch libfuzzer-sys; its evidence says so
# ("driver": "smoke") and the release gate does not accept it for a release.
#
# CARGO is the cargo wrapper to build with (default: cargo). The libFuzzer build sets
# RUSTUP_TOOLCHAIN=nightly itself, because the wrapper cannot pass `+nightly`.
set -euo pipefail
export LC_ALL=C

SECONDS_EACH="${1:?usage: fuzz_all.sh SECONDS_PER_TARGET [EVIDENCE_FILE]}"
if [[ ! "$SECONDS_EACH" =~ ^[1-9][0-9]*$ ]]; then
  echo "SECONDS_PER_TARGET must be a positive integer" >&2
  exit 2
fi
EVIDENCE="${2:-fuzz/evidence.json}"
ONLY_TARGET="${3:-}"
DRIVER="${DRIVER:-libfuzzer}"
CARGO="${CARGO:-cargo}"
ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

START_COMMIT="$(git rev-parse HEAD)"
START_CLEAN=true
START_STATUS_BEFORE="$(git status --porcelain)"
tree_snapshot() {
  python3 - <<'PY'
import hashlib
import os
from pathlib import Path
import subprocess

paths = subprocess.check_output(
    ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"]
).split(b"\0")
digest = hashlib.sha256()
for raw in sorted(p for p in paths if p):
    path = Path(os.fsdecode(raw))
    digest.update(raw + b"\0")
    try:
        value = os.fsencode(os.readlink(path)) if path.is_symlink() else path.read_bytes()
    except FileNotFoundError:
        value = b"<missing>"
    digest.update(hashlib.sha256(value).digest())
print(digest.hexdigest())
PY
}
START_TREE="$(tree_snapshot)"
START_STATUS_AFTER="$(git status --porcelain)"
if [ -n "$START_STATUS_BEFORE$START_STATUS_AFTER" ]; then
  START_CLEAN=false
fi

LIMITS=(systemd-run --user --scope -q -p MemoryMax=3G -p MemorySwapMax=0 taskset -c 0-1)
OUT="$(mktemp -d "${TMPDIR:-/tmp}/faris-fuzz-run.XXXXXX")"
trap 'rm -rf "$OUT"' EXIT

TARGETS=()
for f in fuzz/fuzz_targets/*.rs; do TARGETS+=("$(basename "$f" .rs)"); done
if [ -n "$ONLY_TARGET" ]; then
  if [[ ! " ${TARGETS[*]} " == *" $ONLY_TARGET "* ]]; then
    echo "unknown fuzz target: $ONLY_TARGET" >&2
    exit 2
  fi
  TARGETS=("$ONLY_TARGET")
fi

python3 scripts/fuzz_seed.py --out fuzz/corpus > /dev/null
mkdir -p fuzz/artifacts fuzz/corpus/study_file

if [ "$DRIVER" = libfuzzer ]; then
  export RUSTUP_TOOLCHAIN=nightly
  TOOLCHAIN="$(rustc --version)"
  $CARGO fuzz build --fuzz-dir fuzz
  BIN_DIR="fuzz/target/$(rustc -vV | sed -n 's/^host: //p')/release"
else
  TOOLCHAIN="$(rustc --version) (smoke driver)"
  $CARGO build --release --manifest-path fuzz/entries/Cargo.toml --bin smoke
  SMOKE="${CARGO_TARGET_DIR:-fuzz/entries/target}/release/smoke"
fi

# One real study file as a seed: the writer is the only honest source of a valid one.
$CARGO build --release --manifest-path fuzz/entries/Cargo.toml --bin study_seed
"${CARGO_TARGET_DIR:-fuzz/entries/target}/release/study_seed" fuzz/corpus/study_file/real-study.faris

for t in "${TARGETS[@]}"; do
  echo "== $t ($SECONDS_EACH s)"
  art="fuzz/artifacts/$t"
  mkdir -p "$art" "fuzz/corpus/$t"
  before="$(find "$art" -type f ! -name '*.log' | wc -l)"
  start=$SECONDS
  cpu_millis=0
  status=0
  execs=0
  while { [ "$cpu_millis" -lt "$((SECONDS_EACH * 1000))" ] || [ "$((SECONDS - start))" -lt "$SECONDS_EACH" ]; } && [ "$((SECONDS - start))" -lt "$((SECONDS_EACH * 2 + 300))" ]; do
    cpu_remaining=$(((SECONDS_EACH * 1000 - cpu_millis + 999) / 1000))
    wall_remaining=$((SECONDS_EACH - (SECONDS - start)))
    remaining=$((cpu_remaining > wall_remaining ? cpu_remaining : wall_remaining))
    chunk=$((remaining < 300 ? remaining : 300))
    if [ "$DRIVER" = libfuzzer ]; then
      "${LIMITS[@]}" /usr/bin/time -f '%U %S' -o "$OUT/$t.cpu" "$BIN_DIR/$t" "fuzz/corpus/$t" -max_total_time="$chunk" -rss_limit_mb=2048 \
        -timeout=10 -print_final_stats=1 -artifact_prefix="$art/" >> "$OUT/$t.log" 2>&1 || status=$?
      current_execs="$(sed -n 's/^stat::number_of_executed_units: *//p' "$OUT/$t.log" | tail -1)"
    else
      "${LIMITS[@]}" /usr/bin/time -f '%U %S' -o "$OUT/$t.cpu" "$SMOKE" "$t" "$chunk" "fuzz/corpus/$t" "$art" >> "$OUT/$t.log" 2>&1 || status=$?
      current_execs="$(sed -n 's/.*"executions": *\([0-9]*\).*/\1/p' "$OUT/$t.log" | tail -1)"
    fi
    execs=$((execs + ${current_execs:-0}))
    read -r user_cpu system_cpu extra < "$OUT/$t.cpu"
    [ -z "${extra:-}" ]
    measured_millis="$(awk -v u="$user_cpu" -v s="$system_cpu" \
      'BEGIN { if (u !~ /^[0-9]+([.][0-9]+)?$/ || s !~ /^[0-9]+([.][0-9]+)?$/) exit 2; n=int((u+s)*1000); if (n < 1) exit 3; print n }')"
    cpu_millis=$((cpu_millis + measured_millis))
    [ "$status" -eq 0 ] || break
  done
  after="$(find "$art" -type f ! -name '*.log' | wc -l)"
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$t" "$((SECONDS - start))" "${execs:-0}" "$((after - before))" "$status" "$((cpu_millis / 1000))" >> "$OUT/results.tsv"
  tail -3 "$OUT/$t.log"
done

END_COMMIT="$(git rev-parse HEAD)"
END_TREE="$(tree_snapshot)"
END_STATUS="$(git status --porcelain)"
DIRTY=false
if [ "$START_CLEAN" = false ] || [ "$START_COMMIT" != "$END_COMMIT" ] || [ "$START_TREE" != "$END_TREE" ] || [ -n "$END_STATUS" ]; then
  DIRTY=true
fi
python3 - "$OUT/results.tsv" "$EVIDENCE" "$SECONDS_EACH" "$DRIVER" "$TOOLCHAIN" "$DIRTY" "$START_COMMIT" <<'PY'
import json, sys
from datetime import datetime, timezone
from pathlib import Path
results, evidence, each, driver, toolchain, dirty, commit = sys.argv[1:]
checks = []
for line in open(results):
    name, seconds, execs, crashes, status, cpu_seconds = line.rstrip("\n").split("\t")
    clean = int(crashes) == 0 and int(status) == 0 and int(cpu_seconds) >= int(each)
    checks.append({
        "name": name, "verdict": "PASS" if clean else "FAIL", "seconds": int(seconds),
        "executions": int(execs), "crashes": int(crashes), "exit_status": int(status),
        "cpu_seconds": int(cpu_seconds),
    })
doc = {
    "schema_version": "faris-fuzz-evidence/v0.2", "commit": commit, "dirty": dirty == "true",
    "driver": driver, "toolchain": toolchain, "seconds_per_target_requested": int(each),
    "finished": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"), "checks": checks,
}
Path(evidence).parent.mkdir(parents=True, exist_ok=True)
with open(evidence, "w") as f:
    json.dump(doc, f, indent=2)
    f.write("\n")
bad = [c["name"] for c in checks if c["verdict"] != "PASS"]
print(f"{len(checks)} targets, {sum(c['executions'] for c in checks)} executions, "
      f"{sum(c['crashes'] for c in checks)} crashes; evidence in {evidence}")
sys.exit(1 if bad else 0)
PY
