<!-- kind: how-to -->
# Development

Read `AGENTS.md`, the demo scope, and the crate whose behavior changes. Keep
scientific operations in the shared engine and UI code in the desktop crate.
Prefer one small, demonstrable capability per change.

FARIS source code and documentation use [AGPL-3.0-only](LICENSE). Keep crate
license metadata inherited from the workspace and retain applicable notices
when reusing third-party code or data.

Run:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Every parser of outside bytes has a fuzz target; a new one needs an entry in `fuzz/entries` and a target
file (`docs/FUZZING.md`).

Use tests that establish meaningful behavior: geometry identities, independent
volume checks, conservation, units, event semantics, unsupported-domain handling,
or a named external reference. Future numerical-model changes need appropriate
reference or measured controls; UI polish alone does not need physics tests.

Document what a calculation supports and any consequential limits. Update schema
or model versions when semantics change. Preserve prior run files. Downloaded
scientific inputs and customer data belong in ignored directories, with provenance
and content identity recorded separately.

Numbers that reach a recorded history, ensemble or comparison must be identical
on every platform, because `verify.sh` recomputes the recorded histories and
compares SHA-256 digests. In `faris-model` and `faris-engine` use
`faris_model::math` (`exp`, `ln`, `powf`, `powi`, `sin`, ...; pure-Rust `libm`)
instead of the `f64` methods of the same names, and never `mul_add`.
`clippy.toml` rejects the std methods; plotting, camera and display-mesh code
may allow them with a stated reason. The `determinism_` tests pin the digests
of a 30-year history and a small ensemble; CI runs them on all four desktop
platforms (`cargo test --release -p faris-engine determinism_`).

## Licence of contributions and sign-off

A contribution is accepted under the licence of the project, AGPL-3.0-only, and under no other terms. By sending one
you state that you wrote it or have the right to submit it under that licence, as set out in the
[Developer Certificate of Origin 1.1](https://developercertificate.org/).

Say so by signing off every commit of a pull request from a fork:

```bash
git commit -s
```

This adds a `Signed-off-by: Your Name <you@example.org>` line, and the name and address must be those of the commit's
author. The check `Rust checks / sign-off` in CI reads the commits of the pull request and fails when one is missing.
Commits by the maintainers on branches of this repository are not covered by the check.

Do not add a third-party file, figure or data set without its licence text and a note in the notices
(`docs/THIRD_PARTY_NOTICES.md`, `docs/NOTICE_COVERAGE.md`). New source files start with the line
`// SPDX-License-Identifier: AGPL-3.0-only` (`#` for Python and shell).

## Golden files

Fixtures that other tests compare against (`crates/*/tests/fixtures`, `references/`, `validation/`) change only with
review. `.github/CODEOWNERS` names the reviewer, and a pull request that changes one of these files says why the new
bytes are right. Regenerating a fixture to make a failing test pass is not a reason.

## Workflows

Pin every GitHub Action by its full commit hash and put the version in a trailing comment
(`uses: actions/checkout@<40 hex digits> # v4`). Find the hash with
`gh api repos/OWNER/REPO/commits/TAG --jq .sha`. Workflows get `contents: read` unless a job proves it needs more.
`scripts/test_workflows.py` fails on an unpinned action.
