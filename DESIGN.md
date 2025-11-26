# Memex - Claude Sessions 知识管理系统

> "Memory Extender" - 让你的 Claude Code 对话历史永不丢失，随时可搜、可问、可沉淀。

---

## 共享代码架构

Memex 与 [Vlaude](../hi/小工具/claude)（跨设备 Claude Code 对话系统）共享核心代码：

```
claude-core (共享包)
├── JSONL 解析
├── 路径编码/解码
├── Session/Message 领域模型
├── 文件监听
└── Claude 格式变更只改这里

     ┌──────────┬──────────┐
     │  Vlaude  │  Memex   │
     │ 实时同步  │ 知识管理  │
     └──────────┴──────────┘
```

**好处**：
- Claude Code JSONL 格式变化时，只需改一个地方
- 两个项目共享同一套解析逻辑
- 减少重复代码和维护成本

**参考实现**：`/Users/higuaifan/Desktop/hi/小工具/claude/packages/shared-core`

---

## 项目背景

### 问题

Claude Code 的对话历史存储在 `~/.claude/projects/` 目录下，存在以下问题：

1. **30 天过期**：本地会话数据最多保留 30 天
2. **难以检索**：数百个 JSONL 文件，grep 大海捞针
3. **知识流失**：复杂工作（逆向、调试）的精妙过程没有沉淀，30 天后消失

### 目标

构建一个本地 daemon 服务，实现：

1. **数据不丢** - 自动备份，突破 30 天限制
2. **能搜到** - 全文索引，快速定位历史对话
3. **搜得准** - 语义搜索，"hook 播放器" 能找到 "Frida ExoPlayer"
4. **能问答** - RAG，直接回答问题而不是返回一堆链接
5. **能沉淀** - 从对话中提炼结构化知识

---

## 核心功能

### Phase 0: 数据落地（最高优先级）

- 扫描 `~/.claude/projects/` 下所有 JSONL 文件
- 解析对话内容
- 存储到本地 SQLite
- 每日增量备份（保留最近 30 天的备份）

### Phase 1: 基础搜索

- SQLite FTS5 全文索引
- 关键词搜索
- 按项目、时间过滤
- HTTP API

### Phase 2: 语义搜索（依赖向量数据库）

- Ollama embedding（本地）
- LanceDB 向量存储
- 混合检索（关键词 + 语义）
- RRF 融合排序

#### Phase 2 详细设计

**增量 Embedding 策略**：

当前采用 **方案 C（定时批量）**，简单可靠：
- 定时任务扫描 `embedded_at IS NULL` 的消息
- 批量生成 embedding 并存入 LanceDB
- 启动时 + 定时（如每小时）触发

**未来优化方向 - 方案 B（Claude Hooks 集成）**：

Claude Code 支持 [hooks](https://docs.anthropic.com/en/docs/claude-code/hooks) 机制，可实现近实时索引：

```json
// ~/.claude/settings.json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "TodoWrite",
        "commands": ["curl -X POST http://localhost:10013/api/embedding/trigger"]
      }
    ],
    "Stop": [
      {
        "commands": ["curl -X POST http://localhost:10013/api/embedding/session/$SESSION_ID"]
      }
    ]
  }
}
```

优势：
- 会话结束时自动触发 embedding
- 无需轮询，减少资源消耗
- 用户无感知，后台异步处理

**混合检索架构**：

```
查询 "如何部署"
    │
    ├──────────────────────┬──────────────────────┐
    ▼                      ▼                      │
┌─────────┐          ┌──────────┐                 │
│  FTS5   │          │ LanceDB  │                 │
│ 关键词  │          │  向量    │                 │
└────┬────┘          └────┬─────┘                 │
     │                    │                       │
     ▼                    ▼                       │
[id1, id2, id3]    [id4, id2, id5]               │
     │                    │                       │
     └────────┬───────────┘                       │
              ▼                                   │
        ┌──────────┐                              │
        │ RRF 融合  │  Reciprocal Rank Fusion    │
        └────┬─────┘                              │
             ▼                                    │
     [id2, id1, id4, id3, id5]  ← 融合排序       │
             │                                    │
             ▼                                    │
     SQLite 补全详情 (session/project/context)   │
             │                                    │
             ▼                                    │
        返回结果                                   │
```

**RRF 公式**：`score(d) = Σ 1/(k + rank_i(d))`，k=60

**API 设计**：

```
GET /api/search/semantic?q=xxx&limit=10
    → 混合检索，返回融合排序结果

POST /api/embedding/build
    → 全量构建向量索引

POST /api/embedding/trigger
    → 触发增量 embedding（供 hooks 调用）

GET /api/embedding/stats
    → 索引状态（总数/已索引/待处理）
```

### Phase 3: MCP 集成

✅ **已完成**

实现了 MCP (Model Context Protocol) Server，让 Claude Code 可以直接查询历史对话。

**实现的工具**：

1. `search_history` - 全文搜索历史对话
   - 支持 FTS 搜索（向量搜索待 Ollama 集成后支持）
   - 自动通过 cwd 匹配项目
   - 返回 messageIndex 方便定位

2. `get_session` - 获取会话详情
   - 支持 ID 前缀匹配
   - 支持分页
   - 支持会话内搜索，自动定位到匹配位置

3. `get_recent_sessions` - 最近会话列表
   - 可按项目过滤
   - 显示预览和基本信息

4. `list_projects` - 列出所有项目
   - 显示会话数统计

**配置方式**：

在 `~/.claude/settings.json` 中添加：
```json
{
  "mcpServers": {
    "memex": {
      "command": "node",
      "args": ["/path/to/memex/dist/mcp/index.js"]
    }
  }
}
```

详见 [MCP.md](./MCP.md)

### Phase 4: RAG 问答

✅ **已完成**

- 基于历史知识回答问题
- 引用来源
- Web UI "Ask AI" 功能

### ~~Phase 5: 知识提炼~~

❌ **暂不实现**

**原因**：
- Phase 4 RAG 已能实时总结和回答问题，覆盖了主要使用场景
- 预先提炼知识的增量价值有限（RAG 更灵活）
- 维护成本高（知识过时、质量控制）
- 性价比不高

**可能的替代方向**（按需实现）：
- Session 导出为 Markdown/PDF
- 简单的标签/收藏系统
- Claude Hooks 集成（近实时索引）

---

## 技术架构

```
┌─────────────────────────────────────────────────────────────┐
│                       memex-daemon                          │
│                      (NestJS 服务)                          │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│   ┌─────────────┐                                           │
│   │   Watcher   │  监听 ~/.claude/projects/                 │
│   │  (chokidar) │  检测新增/修改的 JSONL                     │
│   └──────┬──────┘                                           │
│          │                                                  │
│          ▼                                                  │
│   ┌─────────────┐                                           │
│   │   Parser    │  解析 JSONL                               │
│   │             │  增量处理（记录已处理的 offset）            │
│   └──────┬──────┘                                           │
│          │                                                  │
│          ▼                                                  │
│   ┌─────────────┐                                           │
│   │   Storage   │  SQLite + FTS5                            │
│   │             │  (Phase 2+ 加 LanceDB)                    │
│   └──────┬──────┘                                           │
│          │                                                  │
│          ▼                                                  │
│   ┌─────────────┐                                           │
│   │    API      │  HTTP (localhost)                         │
│   │             │  Phase 3+ 加 MCP                          │
│   └─────────────┘                                           │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

---

## 技术选型

| 组件 | 选择 | 理由 |
|------|------|------|
| 框架 | NestJS | 成熟的 DDD 架构，熟悉的技术栈 |
| 通信 | HTTP | 简单，调试方便 |
| 主存储 | SQLite | 单文件，便携，FTS5 全文搜索 |
| 向量存储 | LanceDB | 纯本地，零配置（Phase 2）|
| Embedding | Ollama (nomic-embed-text) | 本地运行，隐私安全（Phase 2）|
| 知识提炼 | Ollama (qwen3:8b) | 本地 LLM（Phase 5）|

---

## 数据目录结构

```
~/memex-data/                      # 数据目录
├── memex.db                       # SQLite 主数据库
├── memex.db-wal                   # WAL 日志
├── backups/                       # JSONL 备份
│   ├── 2024-11-24/               # 按日期组织
│   │   └── {project-hash}/
│   │       └── {session-id}.jsonl
│   └── 2024-11-23/
└── vectors/                       # LanceDB 数据（Phase 2）
```

---

## 备份策略

- **触发时机**：每日定时 + 服务启动时
- **策略**：增量备份（对比 mtime/size，只处理变化的文件）
- **保留**：最近 30 天的备份，自动清理旧数据
- **存储**：复制原始 JSONL 到备份目录

---

## 项目结构（NestJS + DDD）

```
memex/
├── src/
│   ├── main.ts
│   ├── app.module.ts
│   │
│   ├── context/
│   │   └── brain/
│   │       ├── brain.context.ts
│   │       │
│   │       ├── api/
│   │       │   └── controllers/
│   │       │
│   │       ├── application/
│   │       │   └── services/
│   │       │       ├── backup.service.ts
│   │       │       ├── parser.service.ts
│   │       │       ├── indexer.service.ts
│   │       │       └── search.service.ts
│   │       │
│   │       ├── domain/
│   │       │   ├── entities/
│   │       │   └── repositories/
│   │       │
│   │       └── infrastructure/
│   │           ├── sqlite/
│   │           └── watcher/
│   │
│   └── common/
│
├── data/                          # 运行时数据（.gitignore）
├── package.json
├── tsconfig.json
└── nest-cli.json
```

---

## Claude Code JSONL 格式参考

```jsonl
{"type":"file-history-snapshot","messageId":"xxx","snapshot":{...}}
{"parentUuid":null,"type":"user","message":{"role":"user","content":"..."},"uuid":"xxx","timestamp":"..."}
{"parentUuid":"xxx","type":"message","message":{"role":"assistant","content":[...]},...}
```

关键字段：
- `type`: "user" | "message" (assistant) | "file-history-snapshot"
- `message.role`: "user" | "assistant"
- `message.content`: 消息内容（字符串或数组）
- `timestamp`: ISO 时间戳
- `uuid` / `parentUuid`: 消息链

---

## 开发计划

| Phase | 内容 | 状态 |
|-------|------|------|
| **前置** | 重构 shared-core，抽出公共代码 | ✅ 完成 |
| 0 | 项目初始化 + 数据采集 + 备份 | ✅ 完成 |
| 1 | SQLite + FTS5 + HTTP API | ✅ 完成 |
| 2 | Embedding + LanceDB + 混合搜索 | ✅ 完成 |
| 3 | MCP 集成 | ✅ 完成 |
| 4 | RAG 问答 | 待开始 |
| 5 | 知识提炼 | 待开始 |

---

## 前置任务：重构 shared-core

在 memex 开发前，需要先在 Vlaude 项目中重构 shared-core，把 data-collector 的核心逻辑抽出来。

**位置**: `/Users/higuaifan/Desktop/hi/小工具/claude/packages/shared-core`

**需要抽出的模块**：

```
shared-core/
├── src/
│   ├── domain/                    # 已有
│   │   ├── ClaudeSessionAR.ts
│   │   ├── ClaudeMessageEntity.ts
│   │   └── ...
│   │
│   ├── parser/                    # 需要从 data-collector 抽出
│   │   ├── jsonl-parser.ts        # JSONL 解析、cwd 提取
│   │   └── message-filter.ts      # 消息类型过滤
│   │
│   ├── codec/                     # 需要从 data-collector 抽出
│   │   └── path-codec.ts          # 路径编码/解码
│   │
│   └── scanner/                   # 需要从 data-collector 抽出
│       ├── project-scanner.ts     # 项目扫描
│       └── session-reader.ts      # Session 消息读取
```

**完成后**：
- Vlaude 的 data-collector 改为调用 shared-core
- Memex 通过本地路径引用 shared-core

**Memex package.json 引用方式**：
```json
{
  "dependencies": {
    "@anthropic/shared-core": "file:../../hi/小工具/claude/packages/shared-core"
  }
}
```

---

## 从 Vlaude 复用的关键实现

### 路径编码规则

Claude Code 将项目路径编码为目录名：
```
/Users/xxx/project → -Users-xxx-project
```

### JSONL 快速解析

使用 grep 提取 cwd，不需要解析整个文件：
```bash
head -n 10 "${jsonlFilePath}" | grep -o '"cwd":"[^"]*"' | head -1
```

### 消息类型过滤

跳过 SDK 内部消息类型：
- `queue-operation`
- `checkpoint`
- `file-history-snapshot`
- `summary`

### 路径映射缓存

维护 `真实路径 → 编码目录名` 的映射缓存，避免每次都扫描目录。

### 增量检测

记录已处理文件的 `mtime` 和 `size`，只处理变化的文件。

---

## 相关链接

- Claude Code 数据位置: `~/.claude/projects/`
- **共享代码**: `/Users/higuaifan/Desktop/hi/小工具/claude/packages/shared-core`
- **Vlaude 项目**: `/Users/higuaifan/Desktop/hi/小工具/claude`
- 社区参考（不使用）:
  - [claude-code-history-mcp](https://github.com/tim0120/claude-code-history-mcp)
  - [meta-cc](https://github.com/yaleh/meta-cc)
