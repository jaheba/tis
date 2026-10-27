.PHONY: test format-rs lint-rs

test:
	cargo test

format-rs:
	cargo fmt --all

lint-rs:
	cargo clippy --all-targets -- -D warnings
