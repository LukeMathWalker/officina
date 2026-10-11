#!/usr/bin/env bash
set -euo pipefail
cargo build --bin cheadergen
TARGET_DIR="$(cargo metadata --no-deps --format-version 1 | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')"
echo "CHEADERGEN_BIN=$TARGET_DIR/debug/cheadergen" >> "$NEXTEST_ENV"
