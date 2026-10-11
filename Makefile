.PHONY: check occt-check app validate doctor

check:
	cargo fmt --all --check
	cargo clippy --workspace --exclude faris-occt --all-targets -- -D warnings
	cargo test --workspace --exclude faris-occt

# Builds OpenCASCADE from source on the first run (about 90 minutes); set FARIS_OCCT_PREFIX to reuse it.
occt-check:
	cargo clippy -p faris-occt --all-targets -- -D warnings
	cargo test -p faris-occt

validate:
	cargo run -p faris-cli -- validate scenarios/arc-inspired/scenario.json

doctor:
	cargo run -p faris-cli -- doctor

app:
	cargo run -p faris-app
