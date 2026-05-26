#!/bin/bash
set -euo pipefail

PG_VERSION="${PG_VERSION:-18}"

echo "Checking formatting..."
cargo fmt --all -- --check

echo "Running clippy..."
cargo clippy --all-targets --features "pg${PG_VERSION}" -- -D warnings

echo "Running tests..."
cargo pgrx test "pg${PG_VERSION}" --no-default-features --features "pg${PG_VERSION}"
