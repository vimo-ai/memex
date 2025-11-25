#!/bin/bash

# Memex MCP Server 配置脚本
# 自动将 Memex MCP 配置添加到 Claude Code settings

set -e

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MCP_ENTRY="$PROJECT_DIR/dist/mcp/index.js"
CLAUDE_SETTINGS="$HOME/.claude/settings.json"

echo "🔧 Memex MCP Server 配置脚本"
echo ""

# 检查 MCP 入口文件是否存在
if [ ! -f "$MCP_ENTRY" ]; then
  echo "❌ MCP Server 未构建，请先运行："
  echo "   cd $PROJECT_DIR"
  echo "   pnpm install"
  echo "   pnpm run build:mcp"
  exit 1
fi

echo "✅ 找到 MCP Server: $MCP_ENTRY"
echo ""

# 检查 Claude settings 文件
if [ ! -f "$CLAUDE_SETTINGS" ]; then
  echo "📝 创建 Claude settings 文件..."
  mkdir -p "$(dirname "$CLAUDE_SETTINGS")"
  echo '{}' > "$CLAUDE_SETTINGS"
fi

# 备份原配置
BACKUP_FILE="$CLAUDE_SETTINGS.backup.$(date +%Y%m%d%H%M%S)"
cp "$CLAUDE_SETTINGS" "$BACKUP_FILE"
echo "💾 已备份原配置到: $BACKUP_FILE"
echo ""

# 读取现有配置
CURRENT_CONFIG=$(cat "$CLAUDE_SETTINGS")

# 检查是否已有 memex 配置
if echo "$CURRENT_CONFIG" | grep -q '"memex"'; then
  echo "⚠️  检测到已有 memex 配置，将覆盖"
fi

# 生成新配置（使用 jq 或手动拼接）
if command -v jq >/dev/null 2>&1; then
  # 使用 jq 合并配置
  echo "$CURRENT_CONFIG" | jq --arg entry "$MCP_ENTRY" \
    '.mcpServers.memex = {
      "command": "node",
      "args": [$entry]
    }' > "$CLAUDE_SETTINGS"
else
  # 手动拼接（简单实现，假设原配置是有效 JSON）
  cat > "$CLAUDE_SETTINGS" <<EOF
{
  "mcpServers": {
    "memex": {
      "command": "node",
      "args": ["$MCP_ENTRY"]
    }
  }
}
EOF
fi

echo "✅ 已添加 Memex MCP 配置到 $CLAUDE_SETTINGS"
echo ""
echo "📋 配置内容："
echo "---"
cat "$CLAUDE_SETTINGS"
echo "---"
echo ""
echo "🎉 配置完成！请重启 Claude Code 使配置生效。"
echo ""
echo "💡 测试 MCP Server："
echo "   在 Claude Code 中尝试以下命令："
echo "   - 搜索历史对话: '搜索我之前关于 NestJS 的讨论'"
echo "   - 查看最近会话: '显示最近的对话'"
echo "   - 列出项目: '列出所有项目'"
