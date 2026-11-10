# ACTINV source patches carried by FARIS

These are patches for the separate ACTINV source checkout, not vendored ACTINV code. Apply them from the ACTINV repository root only when the target source-file hash matches the receipt below. FARIS does not silently rewrite an installed ACTINV package or its generated libraries.

## 0001: Preserve MF=1/MT=451 text during numeric normalization

- Target: `crates/actinv-data/src/normalize.rs`
- Expected pre-patch SHA-256: `7041f7a53856f6409ac8d95fd44f683a5c18f496537339668d1953bd49261e6d`
- Patch: `0001-preserve-mf1-mt451-text.patch`
- Change: read `NWD` from the fourth MF=1/MT=451 control record, apply the existing numeric repair independently to the four control records and the later numeric directory, and preserve the `NWD` A66 TEXT records byte-for-byte. If `NWD` is absent or inconsistent with the section length, preserve that entire section and record the fail-closed diagnostic.
- Regression: the patch adds an ACTINV unit test that proves descriptive TEXT bytes survive normalization while numeric NaN fields in the MF=1/MT=451 directory and reaction data still use the existing repair.
- Validation: in the isolated snapshot at `runs/continuation/actinv-normalizer-check-v1/actinv`, the focused regression passed (1 passed, 0 failed; 256 filtered out), and `cargo test --locked -p actinv-data --lib normalize::tests` passed (5 passed, 0 failed, 1 ignored; 251 filtered out). Logs are `runs/continuation/checks/actinv-normalizer-check-v1.log` and `runs/continuation/checks/actinv-normalizer-suite-v1.log`. No test was run against or modified the original ACTINV checkout.
- Evidence: the source warnings in H1, H2, H3 and He3 ENDF/B-VIII.1 files all fall in the TEXT record block. Existing built libraries and run artifacts remain immutable; rebuild future candidate libraries from unchanged source bytes after applying and validating the patch.

## License and attribution

This patch modifies ACTINV source code, Copyright (c) 2026 Connor Avila / Avila Labs. ACTINV is dual-licensed under MIT or Apache-2.0; this patch is distributed under the MIT License. The applicable MIT notice and terms are reproduced in `LICENSE-MIT` in this directory. The patch contains no nuclear-data inputs or generated libraries.
