set shell := ["bash", "-euo", "pipefail", "-c"]

# List the recipes.
default:
    @just --list

# Format the Rust code.
fmt:
    cargo fmt --all

# Lint with the flags CI uses.
clippy:
    cargo clippy --locked --all-targets -- -D warnings

# Check formatting and lints without changing files.
lint: clippy
    cargo fmt --all -- --check

# Run the offline Rust and static script tests (tests that need live Reddit are #[ignore]d).
test:
    cargo test --locked
    node --test tests/js/*.test.js
