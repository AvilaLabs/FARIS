.PHONY: check app validate doctor

check:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo test --workspace

validate:
	cargo run -p faris-cli -- validate scenarios/arc-inspired/scenario.json

doctor:
	cargo run -p faris-cli -- doctor

app:
	cargo run -p faris-app
