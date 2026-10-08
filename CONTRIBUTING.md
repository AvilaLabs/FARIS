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
