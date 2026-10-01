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
