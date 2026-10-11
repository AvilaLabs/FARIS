.PHONY: check occt-check app validate doctor

check:
	cargo fmt --all --check
	cargo clippy --workspace --exclude faris-occt --all-targets -- -D warnings
	cargo test --workspace --exclude faris-occt

# Builds OpenCASCADE from source on the first run (about 90 minutes); set FARIS_OCCT_PREFIX to reuse it.
# Set FARIS_CAD_PYTHON (cadquery 2.8 / OCP 7.9.3.1) to run the native-against-Python reader parity test
# on the corpus, and FARIS_RM_M_STEP as well to run it on RM-M (`-- --ignored`).
occt-check:
	cargo clippy -p faris-occt --all-targets -- -D warnings
	cargo test -p faris-occt
	@if [ -n "$$FARIS_RM_M_STEP" ] && [ -n "$$FARIS_CAD_PYTHON" ]; then cargo test -p faris-occt --test reader_parity -- --ignored; fi

validate:
	cargo run -p faris-cli -- validate scenarios/arc-inspired/scenario.json

doctor:
	cargo run -p faris-cli -- doctor

app:
	cargo run -p faris-app
