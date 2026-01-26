#!/usr/bin/env bash
# ============================================================================
# memex-rs 编译脚本
#
# 编译 memex binary（包含嵌入的 web 前端资源）
#
# 使用方式:
#   ./build.sh              # 编译并部署
#   ./build.sh --no-deploy  # 只编译不部署
#   ./build.sh --no-web     # 跳过 web 构建（使用已有 dist）
# ============================================================================
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_NAME="memex-rs"

# Web 前端目录
MEMEX_WEB_DIR="$SCRIPT_DIR/../web"

DEPLOY=true
BUILD_WEB=true

for arg in "$@"; do
    case $arg in
        --no-deploy)
            DEPLOY=false
            ;;
        --no-web)
            BUILD_WEB=false
            ;;
    esac
done

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

# 构建 Web 前端（嵌入到 binary）
if [ "$BUILD_WEB" = true ]; then
    if [ -d "$MEMEX_WEB_DIR" ] && [ -f "$MEMEX_WEB_DIR/package.json" ]; then
        log_info "Building web frontend..."
        cd "$MEMEX_WEB_DIR"

        # 检查 pnpm
        if ! command -v pnpm &> /dev/null; then
            log_warn "pnpm not found, trying npm..."
            npm install
            npm run build
        else
            pnpm install
            pnpm build
        fi

        if [ -d "$MEMEX_WEB_DIR/dist" ]; then
            log_success "Web frontend built: $MEMEX_WEB_DIR/dist"
        else
            log_error "Web build failed: dist directory not found"
            exit 1
        fi
    else
        log_warn "Web directory not found: $MEMEX_WEB_DIR, skipping web build"
    fi
fi

# 编译 Rust binary（指定 target-dir 避免被 workspace 覆盖）
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
    # 部署到 ~/.vimo/bin/
    ETERM_BIN="$HOME/.vimo/bin"
    log_info "Installing to $ETERM_BIN..."
    mkdir -p "$ETERM_BIN"
    cp "$BINARY" "$ETERM_BIN/"
    chmod +x "$ETERM_BIN/memex"
    log_success "Deployed"
elif [ "$DEPLOY" = false ]; then
    log_info "Skipping deploy (--no-deploy)"
fi

log_success "Done"
