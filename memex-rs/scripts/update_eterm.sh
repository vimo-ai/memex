#!/usr/bin/env bash
# ============================================================================
# 更新 ETerm 中的 Memex 二进制
# ============================================================================
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
ETERM_ROOT="/Users/higuaifan/Desktop/hi/小工具/english"

# MemexKit 插件目录
MEMEXKIT_LIB="$ETERM_ROOT/Plugins/MemexKit/Lib"

echo "Building Memex (arm64, cli mode)..."
cd "$PROJECT_DIR"

# cli feature（HTTP 服务，SharedDb 现为必须依赖）
cargo build --release --features cli

BINARY="$PROJECT_DIR/target/release/memex"

if [ ! -f "$BINARY" ]; then
    echo "Error: binary not found at $BINARY"
    exit 1
fi

# Copy to MemexKit plugin
echo "Copying to MemexKit..."
mkdir -p "$MEMEXKIT_LIB"
cp "$BINARY" "$MEMEXKIT_LIB/"
chmod +x "$MEMEXKIT_LIB/memex"

echo ""
echo "✅ Done! Updated: $MEMEXKIT_LIB/memex"
echo "   Size: $(du -h "$MEMEXKIT_LIB/memex" | cut -f1)"
echo ""
echo "Next: Rebuild ETerm or restart to load the new binary"
