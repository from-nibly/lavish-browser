#!/usr/bin/env bash
set -euo pipefail
cargo fmt --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked -p lavish-browser-zellij --target wasm32-wasip1
