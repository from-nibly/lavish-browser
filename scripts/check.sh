#!/usr/bin/env bash
set -euo pipefail
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p lavish-browser-zellij --target wasm32-wasip1
