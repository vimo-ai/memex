# CLAUDE.md

## 项目简介

Memex 是 Claude Code 会话历史管理系统（Rust 实现），解决本地数据 30 天过期、历史对话难检索、知识无沉淀的问题。

## 架构

V2 读写分离：文件监听和写入由 vimo-agent 统一负责，memex-rs 通过 HTTP API 被动触发 compact 和向量索引。

两种运行模式：
- **CLI 模式**（默认）：本地运行，含文件监听、定时任务、Web UI、MCP
- **Server 模式**：接收客户端 push，提供跨人搜索，不跑 LLM/embedding

## 技术栈

- **语言**: Rust
- **HTTP**: Axum + tower-http
- **存储**: SQLite + FTS5（文本）、LanceDB（向量）
- **Embedding**: BGE-M3（Ollama 本地，1024 维）
- **LLM**: Ollama（compact 用 Qwen3 0.6B，knowledge 用强模型）
- **协议**: MCP（JSON-RPC 2.0）、HTTP API
- **共享依赖**: ai-cli-session-db（reader + search + sync）

## 核心模块

| 模块 | 说明 |
|------|------|
| `compact/` | LLM 多层压缩：L0 原文 → L1 Observations → L2 Talk Summary → L3 Session Summary |
| `knowledge/` | L4 知识结晶：从 L0 提取结构化知识（集群/节点/关系），用强模型，独立于 compact |
| `search/` | 混合检索：FTS5 + 向量搜索 + RRF 融合，支持 L0-L3 多级别 |
| `vector/` | LanceDB 向量存储 |
| `embedding/` | BGE-M3 embedding |
| `rag/` | RAG 问答：混合检索 → 上下文构建 → LLM 生成 |
| `mcp/` | MCP 工具：search_history / get_session / get_recent_sessions / list_projects |
| `server/` | Server 模式：ingest / register / search |
| `db_reader/` | 只读数据库层（替代 SharedDbAdapter） |
| `api/` | HTTP API 路由 |
| `backup/` | 增量备份 |
| `archive/` | 分层归档（daily → weekly → monthly → yearly） |
| `indexer/` | 向量索引器 |
| `inject/` | 上下文注入 |

## Compact 设计

原文始终保留，压缩层可选、可重做：
- **L0**: 原文（messages 表）
- **L1**: Observations — 每个工具调用一个（0.6B 模型）
- **L2**: Talk Summary — 每轮对话一个（0.6B 模型）
- **L3**: Session Summary — 整个会话一个，依赖 L2（0.6B 模型）
- **L4**: Knowledge — 结构化知识提取（强模型，独立管线）

## MCP 设计原则

- 精简输出：只返回 AI 需要的字段，减少 token 消耗
- 渐进披露：列表返回摘要，详情返回完整内容
- 位置导航：`at` 字段定位消息，`around` 模式获取上下文

## 关键路径

| 路径 | 说明 |
|------|------|
| `memex-rs/` | Rust 主项目 |
| `memex-lite/` | 轻量版 |
| `docker/` | Docker 部署配置 |
| `web/` | Web UI |
| `~/.vimo/db/` | 数据库文件 |
| `~/.claude/projects/` | Claude Code 会话数据源 |

## 环境变量

- `PORT` — 服务端口（默认 10013）
- `MEMEX_DATA_DIR` — 数据目录（默认 ~/.vimo/db）
- `MEMEX_WEB_DIR` — Web 静态文件（默认 ~/.vimo/memex/web）
- `OLLAMA_API` — Ollama API URL（默认 http://localhost:11434）

## 开发约定

- 中文注释和文档
- 先实现核心功能，不过度设计
- compact 模块用 0.6B 小模型保持低成本，knowledge 用强模型保证质量
