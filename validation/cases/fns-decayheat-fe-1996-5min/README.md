# FNS Fe decay heat, 1996 five-minute irradiation

This case compares ACTINV's prescribed-spectrum iron decay-heat calculation
with all 20 processed FNS-96 measurements. It is an experimental comparison,
not a blind trial: the same experiment has been used in ACTINV validation. It
does not reconstruct the transport field or establish whole-device shutdown
dose, component heating, safety qualification, or a code ranking.

## Source and attribution

The experiment is the JAEA Fusion Neutronics Source iron sample from the 1996
campaign, distributed by IAEA CoNDERC. The original source is the
[CoNDERC FNS archive](https://www-nds.iaea.org/conderc/fusion/files/fns.zip),
SHA-256 `ba1dd6cb150a4aa3e0d81461054aec7d415ef19d946aba8b9886b31de218252d`.
The archived 5-minute experiment, group-flux, MCNP input-deck, and plot member
hashes are retained in the manifest. The pinned JADE-repackaged ACTINV problem
and measurement table are sourced from
[AvilaLabs/open-benchmarks at commit 1914ed418b597398d758e7c6fa11e83158e2ca21](https://github.com/AvilaLabs/open-benchmarks/commit/1914ed418b597398d758e7c6fa11e83158e2ca21).
Their exact hashes and byte sizes are in `manifest.json`; the files remain
outside this repository. The measurement values are carried in the manifest
with **CC BY 4.0** attribution to JAEA FNS / IAEA CoNDERC and the IAEA-NDS
JADE processing. See the source repository's
[license](https://github.com/AvilaLabs/open-benchmarks/blob/1914ed418b597398d758e7c6fa11e83158e2ca21/LICENSE)
and the [CC BY 4.0 terms](https://creativecommons.org/licenses/by/4.0/).

JADE's [experimental benchmark documentation](https://jade-a-nuclear-data-libraries-vv-tool.readthedocs.io/en/latest/benchmarks/experimental.html#fns-decay-heat-iron-five-minute-irradiation)
defines the processed table as heat in microW/g at cooling times in seconds;
its `Error` column is the reported absolute error divided by the measured
heat. Thus FARIS preserves each relative value and puts `Value * Error` into
the reference row's microW/g error field. No confidence level or covariance
is assigned. ACTINV provides no sampling error, which does not quantify its
model or nuclear-data uncertainty. The case therefore reports C/E descriptively
and the shared scorer keeps rows `INCONCLUSIVE` while covariance is unknown.

## Normalization and scope

The pinned ACTINV problem contains one gram of elemental Fe, a 300-second
irradiation, a descending 709-group FISPACT spectrum, and 20 zero-flux cooling
increments ending at every measured time. The original source deck reports a
total flux of `1.116e10 n/cm2/s`. The raw group-vector sum differs slightly
from this declared total; ACTINV's published input documentation states that
`spectrum.total` scales the vector while preserving shape. This adapter keeps
the source vector and declared total unchanged and records that normalization.
It does not hand-normalize or alter the spectrum. Each table time is cooling
time after shutdown, so it maps to ACTINV absolute time `300 s + cooling time`.
No interpolation, shifting, or measurement omission is allowed.

ACTINV's `heat_W_per_g.total` is converted to microW/g by exactly `1e6`; the
runner checks total heat against alpha + beta + gamma before making a record.
ACTINV's resolved activation library, index, primary and fallback decay-data
hashes are captured from its certificate. The input references TENDL-2025
neutron 709-group activation data, ENDF/B-VIII.0 decay data, and JEFF-3.3
fallback data. No library is downloaded by the runner.

The measured `Error` values have no stated confidence interpretation or
point-to-point covariance in the processed source. The compatibility rule
therefore declares covariance `unknown`: FARIS can preserve reference errors
and calculate C/E, but cannot claim an experimental pass/fail or prediction
uncertainty from this record. This is a prescribed-spectrum inventory
benchmark; transport, field uncertainty and sample geometry are outside its
scope.

## Run and record

Build the manifest only from a local checkout containing the pinned source
commit and verify every source blob:

```sh
python3 validation/cases/fns-decayheat-fe-1996-5min/build_manifest.py \
  --upstream /path/to/open-benchmarks
```

The read-only plan verifies the pinned source tree and bounded input sizes:

```sh
python3 validation/cases/fns-decayheat-fe-1996-5min/run_case.py plan \
  --upstream /path/to/open-benchmarks
```

To execute, provide an installed ACTINV binary and an already installed data
root. The runner validates then runs one case with one worker and a 180-second
timeout. It refuses an existing work directory or record file; source inputs,
nuclear data and solver outputs remain external to Git.

```sh
python3 validation/cases/fns-decayheat-fe-1996-5min/run_case.py run \
  --upstream /path/to/open-benchmarks \
  --actinv /path/to/actinv \
  --data-root /path/to/actinv-data \
  --work-dir /tmp/faris-fns-fe-1996-5min-run
```

The sealed `faris-run` record uses the existing `validation/scoring.py`
pipeline. It carries the adapter, manifest, source spec, ACTINV executable,
ACTINV result, and resolved nuclear-data hashes. Execution success is not
experimental validation success.

## Recorded execution

The coordinator ran the pinned case on 2026-11-10 UTC with ACTINV Core 1.4.0.
The sealed [run record](records/fns-decayheat-fe-1996-5min.run.json) contains all
20 measured endpoints, the exact executable/result/specification hashes, and
the resolved activation and decay-data identities. Its canonical record digest
is `47e203560e32263a5ddb34052444bcc0df2e1a77d13d6379932eb7567f271720`.
The full 11 MiB solver output and logs remain outside Git.

The shared scorer reports 20 `INCONCLUSIVE` rows, with descriptive pointwise
C/E values from 0.883112 to 0.962269. No point is omitted. Unknown prediction
and reference covariance prevents a compatibility pass/fail; this execution
does not validate transport, shutdown dose, or whole-device heat.
