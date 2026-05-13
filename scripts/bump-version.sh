#!/bin/bash
set -e

# Usage: ./scripts/bump-version.sh <version>
if [ "$#" -ne 1 ]; then
    echo "Usage: $0 <version>"
    exit 1
fi

VERSION=$1
PACKAGE_NAME="spiral"
PG_VERSION="18"
TARBALL="${PACKAGE_NAME}-pg${PG_VERSION}.tar.gz"

echo "Bumping version to ${VERSION}..."

# 1. Build and package locally if needed
if [ ! -f "${TARBALL}" ]; then
    echo "Local tarball not found, building..."
    ./scripts/release.sh
fi

# 2. Calculate SHA256 of the local package
SHA256=$(shasum -a 256 "${TARBALL}" | awk '{print $1}')
echo "Local SHA256: ${SHA256}"

# 3. Update the formula
# Note: For Homebrew, if we are releasing, the URL will be the GitHub one.
# If we want to test locally, we might use the local file URL.
# But usually bump-version prepares for a release.
URL="https://github.com/spiral-database/spiral/archive/refs/tags/v${VERSION}.tar.gz"

./scripts/update-homebrew.sh "${VERSION}" "${URL}" "${SHA256}"

echo "Version bump complete. Ready to tag and push."
