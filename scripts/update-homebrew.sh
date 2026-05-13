#!/bin/bash
set -e

# This script updates the Homebrew formula with the latest tag/version.
# Usage: ./scripts/update-homebrew.sh <version> <url>

if [ "$#" -lt 2 ]; then
    echo "Usage: $0 <version> <url> [sha256]"
    exit 1
fi

VERSION=$1
URL=$2
SHA256=$3

echo "Updating Homebrew formula to version ${VERSION}..."

if [ -z "$SHA256" ]; then
    echo "No SHA256 provided, downloading tarball to calculate..."
    # Download the tarball to calculate SHA256
    TEMP_FILE=$(mktemp)
    curl -L -s -o "${TEMP_FILE}" "${URL}"
    SHA256=$(shasum -a 256 "${TEMP_FILE}" | awk '{print $1}')
    rm "${TEMP_FILE}"
fi

echo "New SHA256: ${SHA256}"

# Update the formula file
FORMULA_FILE="Formula/spiral.rb"

# Update URL (handles both github archive and other URLs)
sed -i.bak "s|url \".*\"|url \"${URL}\"|" "${FORMULA_FILE}"
# Update SHA256
sed -i.bak "s|sha256 \".*\"|sha256 \"${SHA256}\"|" "${FORMULA_FILE}"

rm "${FORMULA_FILE}.bak"

echo "Formula updated successfully!"
