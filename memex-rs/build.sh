#!/usr/bin/env bash
# ============================================================================
# memex-rs 编译脚本
#
# 编译 memex binary 并部署
#
# 使用方式:
#   ./build.sh              # 编译并部署
#   ./build.sh --no-deploy  # 只编译不部署
# ============================================================================
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_NAME="memex-rs"

# 尝试查找 ETerm 目录（支持独立编译和作为子模块）
if [ -d "$SCRIPT_DIR/../../ETerm/Plugins" ]; then
    ETERM_DIR="$SCRIPT_DIR/../../ETerm"
elif [ -d "$SCRIPT_DIR/../../../ETerm/Plugins" ]; then
    ETERM_DIR="$SCRIPT_DIR/../../../ETerm"
else
    ETERM_DIR=""
fi

# Web 前端目录
MEMEX_WEB_SRC="$SCRIPT_DIR/../web"
MEMEX_WEB_DEST="$HOME/.vimo/memex/web"

DEPLOY=true
if [ "$1" = "--no-deploy" ]; then
    DEPLOY=false
fi

# Colors
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

log_info() { echo -e "${BLUE}[$PROJECT_NAME]${NC} $*"; }
log_success() { echo -e "${GREEN}[$PROJECT_NAME]${NC} $*"; }
log_warn() { echo -e "${YELLOW}[$PROJECT_NAME]${NC} $*"; }
log_error() { echo -e "${RED}[$PROJECT_NAME]${NC} $*"; }

# 编译（指定 target-dir 避免被 workspace 覆盖）
log_info "Building memex binary..."
cd "$SCRIPT_DIR"
cargo build --release --features cli --target-dir "$SCRIPT_DIR/target"

BINARY="$SCRIPT_DIR/target/release/memex"

if [ ! -f "$BINARY" ]; then
    log_error "Binary not found: $BINARY"
    exit 1
fi

log_success "Built: $BINARY"

# 部署
if [ "$DEPLOY" = true ]; then
    # 部署到 ~/.vimo/eterm/bin/
    ETERM_BIN="$HOME/.vimo/eterm/bin"
    log_info "Installing to $ETERM_BIN..."
    mkdir -p "$ETERM_BIN"
    cp "$BINARY" "$ETERM_BIN/"
    chmod +x "$ETERM_BIN/memex"

    # 部署前端（如果 dist 存在）
    if [ -d "$MEMEX_WEB_SRC/dist" ]; then
        log_info "Deploying web frontend to $MEMEX_WEB_DEST..."
        mkdir -p "$MEMEX_WEB_DEST"
        rm -rf "$MEMEX_WEB_DEST"/*
        cp -r "$MEMEX_WEB_SRC/dist/"* "$MEMEX_WEB_DEST/"
        log_success "Web frontend deployed"
    else
        log_warn "Web dist not found, skipping frontend deploy"
    fi

    log_success "Deployed"
elif [ "$DEPLOY" = false ]; then
    log_info "Skipping deploy (--no-deploy)"
fi

log_success "Done"
