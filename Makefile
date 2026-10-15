# iced 0.14

check:
	cargo fmt --all --check
	cargo clippy --all-targets -- -D warnings
	cargo test

release:
	cargo build --release

run:
	cargo run --release
