#!/usr/bin/env bash
# Run every fuzz target for the same number of seconds and write the evidence file the release
# gate reads (QA-033, scripts/release_gate.py --fuzz).
#
#   scripts/fuzz_all.sh SECONDS_PER_TARGET [EVIDENCE_FILE]
#
# The release run is 3600 seconds per target (QA-033: at least 1 CPU-hour each, 16 hours in all);
# a quick check is 30 or 60. Every target runs under libFuzzer's own limits (-rss_limit_mb=2048,
# -timeout=10, one job) inside a 3 GB memory cgroup and two CPUs (taskset), so a runaway input
# cannot take the machine down.
#
# DRIVER=libfuzzer (default) builds with `cargo +nightly fuzz` and needs the nightly toolchain,
# cargo-fuzz and the libfuzzer-sys crate. DRIVER=smoke runs the blind mutation driver in
# fuzz/entries instead, for a machine that cannot fetch libfuzzer-sys; its evidence says so
# ("driver": "smoke") and the release gate does not accept it for a release.
#
# CARGO is the cargo wrapper to build with (default: cargo). The libFuzzer build sets
# RUSTUP_TOOLCHAIN=nightly itself, because the wrapper cannot pass `+nightly`.
set -euo pipefail

SECONDS_EACH="${1:?usage: fuzz_all.sh SECONDS_PER_TARGET [EVIDENCE_FILE]}"
EVIDENCE="${2:-fuzz/evidence.json}"
DRIVER="${DRIVER:-libfuzzer}"
CARGO="${CARGO:-cargo}"
ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

LIMITS=(systemd-run --user --scope -q -p MemoryMax=3G -p MemorySwapMax=0 taskset -c 0-1)
OUT="$(mktemp -d "${TMPDIR:-/tmp}/faris-fuzz-run.XXXXXX")"
trap 'rm -rf "$OUT"' EXIT

TARGETS=()
for f in fuzz/fuzz_targets/*.rs; do TARGETS+=("$(basename "$f" .rs)"); done

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
  status=0
  if [ "$DRIVER" = libfuzzer ]; then
    "${LIMITS[@]}" "$BIN_DIR/$t" "fuzz/corpus/$t" -max_total_time="$SECONDS_EACH" -rss_limit_mb=2048 \
      -timeout=10 -jobs=1 -print_final_stats=1 -artifact_prefix="$art/" > "$OUT/$t.log" 2>&1 || status=$?
    execs="$(sed -n 's/^stat::number_of_executed_units: *//p' "$OUT/$t.log" | tail -1)"
  else
    "${LIMITS[@]}" "$SMOKE" "$t" "$SECONDS_EACH" "fuzz/corpus/$t" "$art" > "$OUT/$t.log" 2>&1 || status=$?
    execs="$(sed -n 's/.*"executions": *\([0-9]*\).*/\1/p' "$OUT/$t.log" | tail -1)"
  fi
  after="$(find "$art" -type f ! -name '*.log' | wc -l)"
  printf '%s\t%s\t%s\t%s\t%s\n' "$t" "$((SECONDS - start))" "${execs:-0}" "$((after - before))" "$status" >> "$OUT/results.tsv"
  tail -3 "$OUT/$t.log"
done

DIRTY=false
[ -n "$(git status --porcelain --untracked-files=no)" ] && DIRTY=true
python3 - "$OUT/results.tsv" "$EVIDENCE" "$SECONDS_EACH" "$DRIVER" "$TOOLCHAIN" "$DIRTY" "$(git rev-parse HEAD)" <<'PY'
import json, sys
from datetime import datetime, timezone
results, evidence, each, driver, toolchain, dirty, commit = sys.argv[1:]
checks = []
for line in open(results):
    name, seconds, execs, crashes, status = line.rstrip("\n").split("\t")
    clean = int(crashes) == 0 and int(status) == 0
    checks.append({
        "name": name, "verdict": "PASS" if clean else "FAIL", "seconds": int(seconds),
        "executions": int(execs), "crashes": int(crashes), "exit_status": int(status),
    })
doc = {
    "schema_version": "faris-fuzz-evidence/v0.1", "commit": commit, "dirty": dirty == "true",
    "driver": driver, "toolchain": toolchain, "seconds_per_target_requested": int(each),
    "finished": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"), "checks": checks,
}
with open(evidence, "w") as f:
    json.dump(doc, f, indent=2)
    f.write("\n")
bad = [c["name"] for c in checks if c["verdict"] != "PASS"]
print(f"{len(checks)} targets, {sum(c['executions'] for c in checks)} executions, "
      f"{sum(c['crashes'] for c in checks)} crashes; evidence in {evidence}")
sys.exit(1 if bad else 0)
PY
