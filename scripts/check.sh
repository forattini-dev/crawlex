#!/usr/bin/env bash
# Local quality gate. Now enforces -D warnings (all clippy lints we don't
# explicitly allow at the crate root) plus the full test suite.
set -e
cargo clippy --all-targets --all-features -- -D warnings
cargo test
