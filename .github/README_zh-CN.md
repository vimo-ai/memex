[English](../README.md) | [中文](README_zh-CN.md)

# Memex

AI 编程助手会话历史管理系统，让你的对话永不过期。

## 支持的工具

- ✅ Claude Code
- ✅ Codex CLI
- ✅ OpenCode
- ✅ Gemini CLI

## 为什么需要 Memex？

AI CLI 工具的本地对话数据常常会过期或丢失，导致：
- 重要的技术决策记录丢失
- 历史对话难以检索
- 知识无法沉淀和复用

Memex 解决这些问题：
- 自动备份所有 AI CLI 会话
- 强大的全文和语义搜索
- MCP 协议支持，在 Claude 中直接搜索历史
- REST API 便于集成

## 功能特性

### 数据采集与备份
- 自动扫描所有支持的 AI CLI 数据目录
- 解析各工具的对话格式
- 存储到 SQLite 数据库（FTS5 全文索引）
- 每日增量备份

### 搜索能力
- **全文搜索**: 基于 SQLite FTS5，快速关键词检索
- **语义搜索**: 使用 Ollama + LanceDB 实现向量搜索
- **混合搜索**: RRF 融合排序，同时利用关键词和语义相关性
- **过滤功能**: 按项目、时间范围、Session ID 前缀过滤

### MCP 集成
在 Claude Code 中直接搜索历史对话：
- `search_history` - 搜索对话（FTS/向量/混合）
- `get_session` - 获取会话详情（支持分页）
- `get_recent_sessions` - 获取项目最近会话
- `list_projects` - 列出所有项目

## 技术栈

- **后端**: Rust + Axum
- **数据库**: SQLite + FTS5
- **向量存储**: LanceDB
- **Embeddings**: Ollama (bge-m3)
- **协议**: HTTP + JSON-RPC (MCP)

## 快速开始

### Docker（推荐）

```bash
docker run -d -p 10013:10013 \
  -v ~/.claude:/data/claude \
  -v ~/.vimo:/data/vimo \
  -e VIMO_HOME=/data/vimo \
  -e CLAUDE_PROJECTS_PATH=/data/claude/projects \
  ghcr.io/vimo-ai/memex:latest
```

验证运行状态：

```bash
curl http://localhost:10013/health          # → OK
curl http://localhost:10013/api/stats       # → {"projectCount":...}
```

### 启用语义搜索

需要在宿主机运行 Ollama：

```bash
# 安装 Ollama 并拉取 embedding 模型
ollama serve
ollama pull bge-m3

# 启动 Memex 连接 Ollama
docker run -d -p 10013:10013 \
  -v ~/.claude:/data/claude \
  -v ~/.vimo:/data/vimo \
  -e VIMO_HOME=/data/vimo \
  -e CLAUDE_PROJECTS_PATH=/data/claude/projects \
  -e OLLAMA_API=http://host.docker.internal:11434 \
  ghcr.io/vimo-ai/memex:latest
```

**Linux 提示**: `host.docker.internal` 在 Docker Desktop 上可用。原生 Linux 需使用 `--add-host=host.docker.internal:host-gateway` 或宿主机 IP。

### 从源码构建

```bash
git clone https://github.com/vimo-ai/memex.git
cd memex/memex-rs
cargo build --release
./target/release/memex serve
```

### Memex Lite（零依赖 CLI）

快速搜索，无需启动服务：

```bash
# 构建
cd memex/memex-lite
cargo build --release

# 搜索所有 AI CLI 历史
./target/release/memex search "authentication"

# 按 CLI 类型过滤
./target/release/memex search "bug fix" --source claude

# 列出最近会话
./target/release/memex list -n 10

# 查看指定会话
./target/release/memex view <session-id>

# 显示可用数据源
./target/release/memex sources
```

Memex Lite 直接读取 JSONL 文件，无需数据库，适合：
- 临时快速搜索
- 新机器无需完整配置
- CI/CD 环境
- 资源受限系统

## 配置

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `PORT` | `10013` | HTTP 服务端口 |
| `VIMO_HOME` | `~/.vimo` | 基础数据目录（SQLite、LanceDB、备份） |
| `CLAUDE_PROJECTS_PATH` | `~/.claude/projects` | Claude Code 会话路径 |
| `CODEX_PATH` | `~/.codex` | Codex CLI 会话路径 |
| `OPENCODE_PATH` | `~/.local/share/opencode` | OpenCode 会话路径 |
| `GEMINI_PATH` | `~/.gemini/history` | Gemini CLI 会话路径 |
| `OLLAMA_API` | `http://localhost:11434` | Ollama API 地址 |
| `EMBEDDING_MODEL` | `bge-m3` | Ollama embedding 模型 |
| `ENABLE_AI_CHAT` | `false` | 启用 RAG 问答功能 |
| `CHAT_MODEL` | `qwen3:8b` | Ollama 聊天模型（用于问答） |

## 入门指南

1. **启动 Memex**（见上方快速开始）

2. **验证会话已导入**（启动时自动导入）
   ```bash
   curl http://localhost:10013/api/stats
   # 应显示 sessionCount > 0
   ```

3. **尝试搜索**
   ```bash
   curl "http://localhost:10013/api/search?q=authentication&limit=5"
   ```

4. **配置 MCP**（可选）- 见下方 MCP 配置

> **提示**：如果 `sessionCount` 为 0，手动触发采集：`curl -X POST http://localhost:10013/api/collect`

## MCP 配置

Memex 通过 HTTP 提供 MCP 服务：`http://localhost:10013/api/mcp`

### Claude Code 配置（使用 mcp-remote）

添加到 `~/.claude.json`：

```json
{
  "mcpServers": {
    "memex": {
      "command": "npx",
      "args": ["-y", "mcp-remote", "http://localhost:10013/api/mcp"]
    }
  }
}
```

### 验证 MCP

```bash
curl -X POST http://localhost:10013/api/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

## API 参考

### 健康检查与统计
- `GET /health` - 健康检查
- `GET /api/stats` - 数据库统计

### 项目与会话
- `GET /api/projects` - 列出所有项目
- `GET /api/sessions` - 列出会话
- `GET /api/sessions/{id}` - 获取会话详情
- `GET /api/sessions/{id}/messages` - 获取会话消息

### 搜索
- `GET /api/search?q=...` - 全文搜索
- `GET /api/search/semantic?q=...` - 语义搜索
- `GET /api/search/hybrid?q=...` - 混合搜索

### 数据采集与索引
- `POST /api/collect` - 触发会话采集
- `POST /api/index` - 索引指定会话
- `GET /api/embedding/stats` - 索引统计

### MCP
- `POST /api/mcp` - MCP JSON-RPC 端点
- `GET /api/mcp/info` - MCP 服务信息

## 数据目录

```
~/.vimo/
├── db/
│   ├── ai-cli-session.db    # SQLite 数据库
│   ├── lancedb/             # 向量存储
│   └── backups/             # 数据库备份
```

## 常见问题

### 如何触发数据导入？

服务启动时会自动导入。手动触发：
```bash
curl -X POST http://localhost:10013/api/collect
```

### 语义搜索不工作？

1. 确保 Ollama 正在运行：`ollama serve`
2. 拉取模型：`ollama pull bge-m3`
3. 检查状态：`curl http://localhost:10013/api/embedding/status`

### 没有会话显示？

1. 检查 Claude Code 路径：`ls ~/.claude/projects/`
2. Docker 用户：确保 `CLAUDE_PROJECTS_PATH=/data/claude/projects` 与卷挂载一致
3. 触发采集：`curl -X POST http://localhost:10013/api/collect`

## 文档

完整文档：https://vimoai.dev/docs/memex

- [安装指南](https://vimoai.dev/docs/memex/installation)
- [配置说明](https://vimoai.dev/docs/memex/configuration)
- [API 参考](https://vimoai.dev/docs/memex/api)
- [MCP 工具](https://vimoai.dev/docs/memex/mcp)
- [架构设计](https://vimoai.dev/docs/memex/architecture)

## 许可证

MIT
