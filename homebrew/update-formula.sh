#!/bin/bash
# Update Homebrew Formula with latest release checksums
#
# Usage: ./update-formula.sh <version>
# Example: ./update-formula.sh 0.1.0
#
# This script:
# 1. Downloads checksums from GitHub Release
# 2. Updates memex-lite.rb with correct sha256 values
# 3. Outputs the updated formula (copy to homebrew-tap repo)

set -e

VERSION="${1:-}"
if [[ -z "$VERSION" ]]; then
    echo "Usage: $0 <version>"
    echo "Example: $0 0.1.0"
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
FORMULA="$SCRIPT_DIR/memex-lite.rb"
RELEASE_URL="https://github.com/vimo-ai/memex/releases/download/lite-v${VERSION}"

echo "Fetching checksums for lite-v${VERSION}..."

# Download checksums file
CHECKSUMS=$(curl -sL "${RELEASE_URL}/checksums.txt")

if [[ -z "$CHECKSUMS" ]]; then
    echo "Error: Could not fetch checksums from ${RELEASE_URL}/checksums.txt"
    exit 1
fi

# Extract sha256 values
ARM64_SHA256=$(echo "$CHECKSUMS" | grep "memex-darwin-arm64.tar.gz" | cut -d' ' -f1)
X64_SHA256=$(echo "$CHECKSUMS" | grep "memex-darwin-x64.tar.gz" | cut -d' ' -f1)
LINUX_SHA256=$(echo "$CHECKSUMS" | grep "memex-linux-x64.tar.gz" | cut -d' ' -f1)

echo "ARM64: $ARM64_SHA256"
echo "X64:   $X64_SHA256"
echo "Linux: $LINUX_SHA256"

# Update formula
sed -i.bak \
    -e "s/version \".*\"/version \"${VERSION}\"/" \
    -e "s/PLACEHOLDER_ARM64_SHA256/${ARM64_SHA256}/" \
    -e "s/PLACEHOLDER_X64_SHA256/${X64_SHA256}/" \
    -e "s/PLACEHOLDER_LINUX_SHA256/${LINUX_SHA256}/" \
    "$FORMULA"

# Also handle already-set sha256 (for re-runs)
sed -i.bak \
    -e "s/sha256 \"[a-f0-9]\{64\}\"/sha256 \"${ARM64_SHA256}\"/1" \
    "$FORMULA"

rm -f "$FORMULA.bak"

echo ""
echo "Updated $FORMULA"
echo ""
echo "Next steps:"
echo "1. Create repo: github.com/vimo-ai/homebrew-tap (if not exists)"
echo "2. Copy formula: cp $FORMULA <homebrew-tap>/Formula/memex-lite.rb"
echo "3. Commit and push"
echo ""
echo "Users can then install with: brew install vimo-ai/tap/memex-lite"
