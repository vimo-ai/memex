[English](../README.md) | [中文](README_zh-CN.md)

# Memex

Claude Code 会话历史管理系统，让你的对话永不过期。

## 为什么需要 Memex？

Claude Code 的本地对话数据会在 30 天后过期，导致：
- 重要的技术决策记录丢失
- 历史对话难以检索
- 知识无法沉淀和复用

Memex 解决这些问题：
- ✅ 自动备份所有 Claude Code 会话
- ✅ 强大的全文和语义搜索
- ✅ MCP 协议支持，在 Claude 中直接搜索历史
- ✅ Web UI 浏览和管理会话

## 功能特性

### 数据采集与备份
- 自动扫描 `~/.claude/projects/` 下所有会话
- 解析 JSONL 格式对话内容
- 存储到 SQLite 数据库
- 支持每日增量备份

### 搜索能力
- **全文搜索**: 基于 SQLite FTS5，快速关键词检索
- **语义搜索**: 使用 Ollama + LanceDB 实现语义理解
- **混合检索**: RRF 融合排序，同时利用关键词和语义相关性
- **高级过滤**: 按项目、时间范围、Session ID 前缀过滤

### MCP 集成
支持在 Claude Code 中通过 MCP 协议搜索历史对话：
- `search_history` - 搜索历史对话
- `get_session` - 获取会话详情（支持分页和会话内搜索）
- `get_recent_sessions` - 获取最近的会话
- `list_projects` - 列出所有项目

### Web UI
- Cyberpunk 风格界面
- 项目列表和会话浏览
- Session ID 前缀快速查找
- 支持全文/语义/混合搜索

## 技术栈

- **后端**: NestJS (DDD 架构)
- **数据库**: SQLite + FTS5 (全文搜索)
- **向量存储**: LanceDB
- **Embedding**: Ollama (bge-m3 模型)
- **前端**: Vue 3
- **通信**: HTTP + JSON-RPC (MCP)

## 安装

### 前置要求

1. Node.js >= 18
2. pnpm (推荐)
3. Ollama (用于语义搜索，可选)

```bash
# 安装 Ollama
curl -fsSL https://ollama.com/install.sh | sh

# 拉取 embedding 模型
ollama pull bge-m3
```

### 安装项目

```bash
# 克隆项目
git clone <repository-url>
cd memex

# 安装依赖
pnpm install

# Web UI 依赖
cd web
pnpm install
cd ..
```

## 配置

复制并编辑配置文件：

```bash
cp .env.example .env
```

主要配置项：

```bash
# 服务端口
PORT=10013

# 数据存储目录
MEMEX_DATA_DIR=~/memex-data

# 备份目录
MEMEX_BACKUP_DIR=~/memex-data/backups

# Claude Code 数据源路径
CLAUDE_PROJECTS_PATH=~/.claude/projects

# Ollama API 地址
OLLAMA_API=http://localhost:11434/api

# Embedding 模型
EMBEDDING_MODEL=bge-m3
```

## 运行

### 开发模式

```bash
# 启动后端
pnpm dev

# 启动前端 (新终端)
cd web
pnpm dev
```

### 生产模式

```bash
# 构建后端
pnpm build

# 构建前端
cd web
pnpm build
cd ..

# 启动服务
pnpm start:prod
```

## MCP 配置

Memex 通过 HTTP 协议提供 MCP 服务，配置简单。

### 方式一：mcp-router 配置（推荐）

编辑 mcp-router 配置文件：

```json
{
  "mcpServers": {
    "memex": {
      "type": "http",
      "url": "http://127.0.0.1:10013/api/mcp"
    }
  }
}
```

### 方式二：Claude Code 直接配置

在 Claude Code 的 MCP 设置中添加：

```json
{
  "mcpServers": {
    "memex": {
      "type": "http",
      "url": "http://127.0.0.1:10013/api/mcp"
    }
  }
}
```

### 验证 MCP 连接

启动 Claude Code 后，可以通过以下方式验证：

```
搜索我最近关于 DDD 架构的讨论
```

如果 MCP 配置正确，Claude 会调用 `memex/search_history` 工具进行搜索。

## API 端点

### 项目管理
- `GET /api/projects` - 获取所有项目列表
- `GET /api/projects/:id` - 获取项目详情（包含会话列表）

### 会话管理
- `GET /api/sessions/:id` - 获取会话详情（完整对话内容）
- `GET /api/sessions/search?idPrefix=xxx` - 通过 Session ID 前缀搜索

### 搜索
- `GET /api/search?q=xxx&projectId=yyy` - 全文搜索
  - 查询参数：
    - `q`: 搜索关键词
    - `projectId`: 项目筛选（可选）
    - `startDate`: 开始日期（可选）
    - `endDate`: 结束日期（可选）
    - `limit`: 返回数量限制，默认 20

- `GET /api/search/semantic?q=xxx&mode=hybrid` - 语义搜索
  - 查询参数：
    - `q`: 搜索内容
    - `mode`: 搜索模式
      - `semantic`: 纯语义搜索
      - `hybrid`: 混合搜索（关键词 + 语义）
    - `projectId`: 项目筛选（可选）
    - `limit`: 返回数量限制，默认 10

### RAG 问答
- `POST /api/ask` - 基于历史对话回答问题
  - 请求参数：
    - `question`: 要问的问题
    - `cwd`: 当前工作目录，用于项目过滤（可选）
    - `contextWindow`: 上下文消息数，默认 3（可选）
    - `maxSources`: 最大引用数，默认 5（可选）
  - 响应: `{ answer, sources, model, tokensUsed }`

### MCP
- `POST /api/mcp` - MCP JSON-RPC 端点
- `GET /api/mcp/info` - 获取 MCP 工具信息

## 使用示例

### Web UI

访问 `http://localhost:10013` 即可使用 Web 界面。

主要功能：
- 浏览所有项目和会话
- 通过 Session ID 前缀快速定位
- 全文搜索对话内容
- 语义搜索相关讨论
- 按项目和时间筛选

### 命令行搜索

```bash
# 全文搜索
curl "http://localhost:10013/api/search?q=DDD架构"

# 语义搜索
curl "http://localhost:10013/api/search/semantic?q=如何设计领域模型&mode=hybrid"

# 按项目搜索
curl "http://localhost:10013/api/search?q=bug&projectId=xxx"
```

### MCP 使用

在 Claude Code 中直接问：

```
帮我搜索之前讨论过的关于 NestJS 依赖注入的对话

找一下最近一周的会话，看看我们都做了什么

获取关于数据库设计的完整会话内容
```

## 数据目录结构

```
~/memex-data/
├── memex.db              # SQLite 数据库
├── vectors/              # LanceDB 向量存储
│   └── messages/
└── backups/              # 备份文件
    └── memex-2025-01-15.db
```

## 常见问题

### Q: 如何触发初始数据导入？

A: 服务启动时会自动扫描 `~/.claude/projects/` 并导入所有会话。你也可以通过 API 手动触发：

```bash
curl -X POST http://localhost:10013/api/backup
```

### Q: 语义搜索不工作？

A: 确保：
1. Ollama 服务正在运行：`ollama serve`
2. 已下载模型：`ollama pull bge-m3`
3. `.env` 中 `OLLAMA_API` 配置正确

### Q: 如何清理和重建索引？

A: 删除数据目录后重启服务：

```bash
rm -rf ~/memex-data
pnpm start
```

### Q: MCP 连接失败？

A: 检查：
1. Memex 服务是否运行在 `http://localhost:10013`
2. MCP 配置中的路径是否正确
3. Node.js 版本是否 >= 18

## 开发

### 项目结构

```
memex/
├── src/
│   ├── context/                 # DDD 上下文
│   │   └── brain/              # 核心上下文
│   │       ├── api/            # API 层
│   │       ├── application/    # 应用服务
│   │       ├── domain/         # 领域模型
│   │       └── infrastructure/ # 基础设施
│   └── main.ts                 # 应用入口
├── web/                        # Vue 前端
└── DESIGN.md                   # 架构设计文档
```

### 运行测试

```bash
pnpm test
```

## 路线图

- [x] Phase 0: 数据采集和备份
- [x] Phase 1: SQLite + FTS5 全文搜索
- [x] Phase 2: 语义搜索 (Ollama + LanceDB)
- [x] Phase 3: MCP 集成
- [x] Web UI
- [x] Phase 4: RAG 问答
- [ ] Phase 5: 知识提炼

## 许可证

MIT

## 致谢

感谢 Claude Code 提供了如此优秀的开发体验。
