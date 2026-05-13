#!/bin/bash
set -e

# Configuration
PG_VERSION=${PG_VERSION:-18}
PACKAGE_NAME="spiral"
TARGET_DIR="target/release/${PACKAGE_NAME}-pg${PG_VERSION}"

echo "Building package for PostgreSQL ${PG_VERSION}..."

# Ensure pgrx is initialized (this might take a while if not done)
# cargo pgrx init

# Package the extension
cargo pgrx package --features pg${PG_VERSION}

# Create tarball
OUTPUT_FILE="${PACKAGE_NAME}-pg${PG_VERSION}.tar.gz"
echo "Creating tarball: ${OUTPUT_FILE}"
tar -czf "${OUTPUT_FILE}" -C "${TARGET_DIR}" .

echo "Done! Package created at ${OUTPUT_FILE}"
