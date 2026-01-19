# RFC-001: LLM Compact 层

> 为 Memex 添加 LLM 驱动的会话压缩/摘要功能

## 背景

当前 Memex 存储完整的会话原文，通过 FTS5 + LanceDB 提供检索。但在 MCP 返回结果时，原文往往过长，消耗大量 token。

## 调研总结

### claude-mem

- **架构**：Hook 系统 + @anthropic-ai/claude-agent-sdk 观察者模式
- **工作方式**：实时观察每次工具调用，生成 observation，每轮对话结束生成 session_summary
- **数据模型**：
  - `observations` 表：每个工具调用一个，有 `prompt_number` 字段追踪属于哪一轮
  - `session_summaries` 表：每轮对话一个（不是只有 session 结束），有 `prompt_number`
  - `user_prompts` 表：用户消息
- **搜索能力**：
  - **FTS5 全文搜索**：observations_fts, session_summaries_fts
  - **ChromaDB 向量搜索**：通过 chroma-mcp，每个项目一个 Collection
  - **混合搜索策略**：SQLite 元数据过滤 + Chroma 语义排序
- **向量索引粒度**：字段级拆分（narrative、text、每个 fact 单独向量）
- **注入方式**：**Pull only**（MCP tools: search → timeline → get_observations）
- **时间窗口**：默认 90 天过滤
- **优点**：实现成熟，社区接受度高
- **缺点**：不保留原文（压缩不可逆），只有 Pull 没有 Push 注入
- **Provider 支持**：Claude SDK、Gemini、OpenRouter（含免费模型如 xiaomi/mimo-v2-flash:free）

### claude-user-memory

- **本质**：Prompt 框架，不是数据存储系统
- **核心**：置信度公式 `confidence = base_confidence × time_decay × evidence_factor`
- **存储**：knowledge-core.md（Markdown 文件）
- **不适用于** Memex 的场景

## 目标

1. **原文始终保留**，摘要作为可选增值层
2. **多层可选压缩**：L1 Observations / L2 Talk Summary / L3 Session Summary
3. 复用 LLM 层抽象（ChatProvider trait），支持多种 Provider
4. **核心差异化**：向量匹配智能注入（RAG for Memory）
5. **社区兼容**：保留全量注入作为可选 feature

## Memex vs claude-mem 核心区别

| 维度 | claude-mem | Memex |
|---|---|---|
| **原始数据** | ❌ 不保留 | ✅ JSONL 完整保留 |
| **压缩层** | **必须**生成（否则丢数据） | **可选**生成（随时补） |
| **生成时机** | 实时（不可逆） | 事后（可重做） |
| **出错恢复** | 无法恢复 | 从原文重新生成 |
| **原文搜索** | ❌ 不可能 | ✅ FTS5 全文搜索 |
| **向量搜索** | ✅ ChromaDB (Python MCP) | ✅ LanceDB (Rust 原生) |
| **注入模式** | Pull only（Claude 主动调用） | **Push + Pull**（自动注入 + 主动查询） |
| **时间窗口** | 90 天过滤 | ✅ 无限制（永久 Archive） |
| **重做能力** | ❌ 不可逆 | ✅ 随时从原文重新生成 |

### Memex 的核心竞争力

1. **原文搜索能力**：摘要漏了关键信息？FTS5 搜原文兜底
2. **Push 智能注入**：用户发消息 → 自动匹配历史 → 注入 context（claude-mem 没有）
3. **Archive 永久存档**：跨时间搜索，一年前的项目细节也能找到
4. **可重做性**：换了更好的模型/Prompt，历史摘要可以重新生成

**定位：原文是根基，压缩是增值，搜索是双轨。**

## 架构设计

### 多层可选压缩架构

```
L0: 原文（JSONL）─────────────────────── 始终保留，100% 完整
         │
         ├──→ L1: Observations ────────── 可选（独立）
         │         每个工具调用/操作一个，剪枝合并
         │
         └──→ L2: Talk Summary ────────── 可选（独立）
                   每轮对话一个
                       │
                       └──→ L3: Session Summary ── 依赖 L2
                                 整个会话一个
```

### 层级依赖关系

| 层级 | 依赖 | 输入来源 | 说明 |
|------|------|----------|------|
| L0 原文 | 无 | JSONL 文件 | 始终存在 |
| L1 Observations | 无 | 原文（单个工具调用） | 独立开关 |
| L2 Talk Summary | 无 | 原文（一轮对话） | 独立开关 |
| L3 Session Summary | **L2** | L2 Summaries | 自动开启 L2 |

**为什么 L3 依赖 L2？**
- 一个 Session 可能 50+ 轮对话，原文可能 50k+ tokens
- 直接从原文生成 L3 会超出模型 context 限制
- 基于 L2 汇总：每个 ~200 tokens × 50 = 10k tokens（可控）

### Token 优化策略

| 层级 | 输入来源 | 理由 |
|------|----------|------|
| L1 Observations | 原文 | 单个工具调用本来就不长 |
| L2 Talk Summary | 原文 | 一轮对话通常可控（几千 token） |
| L3 Session Summary | **L2s** | 多轮汇总时原文太长，用 L2 省 token |

### 服务依赖

```
┌─────────────────────────────────────────────────────────────┐
│                      Compact Service                        │
│  - L1/L2/L3 生成调度                                        │
│  - 增量处理（cursor 追踪）                                   │
│  - 向量索引触发                                              │
└─────────────────────┬───────────────────────────────────────┘
                      │ 依赖
                      ▼
┌─────────────────────────────────────────────────────────────┐
│                      LLM Layer                              │
│  ├── ChatProvider trait                                     │
│  └── EmbeddingProvider trait                                │
│                                                             │
│  Providers:                                                 │
│  ├── OllamaProvider (本地，隐私优先)                         │
│  ├── OpenAIProvider (云端，高质量)                           │
│  ├── OpenRouterProvider (多模型，含免费)                     │
│  └── AnthropicProvider (Claude API)                        │
└─────────────────────────────────────────────────────────────┘
```

**注意**：Compact 层不定义自己的 Provider trait，复用 LLM 层的 `ChatProvider`。

### LLM 层重构（前置依赖）

现有 `embedding/mod.rs` 的 `OllamaClient` 需要重构为：

```rust
// memex-rs/src/llm/mod.rs

pub mod provider;
pub mod ollama;
pub mod openai;
pub mod openrouter;

use async_trait::async_trait;

/// Chat Provider trait
#[async_trait]
pub trait ChatProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn chat(&self, messages: &[ChatMessage]) -> anyhow::Result<String>;
    async fn is_available(&self) -> bool;
}

/// Embedding Provider trait
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>>;
    async fn embed_batch(&self, texts: Vec<String>) -> anyhow::Result<Vec<Vec<f32>>>;
    async fn is_available(&self) -> bool;
}

pub struct ChatMessage {
    pub role: String,      // system | user | assistant
    pub content: String,
}
```

### Compact Service

```rust
// memex-rs/src/compact/mod.rs

use crate::llm::ChatProvider;
use crate::domain::{Session, Message};

pub struct CompactResult {
    pub summary: String,
    pub key_points: Vec<String>,
    pub tokens_original: usize,
    pub tokens_summary: usize,
}

pub struct CompactService {
    chat_provider: Arc<dyn ChatProvider>,
    embedding_provider: Arc<dyn EmbeddingProvider>,
    db: Arc<Database>,
}

impl CompactService {
    /// 对会话进行 compact
    pub async fn compact_session(&self, session_id: &str) -> anyhow::Result<CompactResult> {
        // 1. 加载会话消息
        // 2. 构建 prompt
        // 3. 调用 ChatProvider
        // 4. 解析结果
        // 5. 存储摘要
        // 6. 生成 embedding 并索引
    }
}
```

### Compact Prompt 设计

```rust
const COMPACT_SYSTEM_PROMPT: &str = r#"
你是一个会话摘要助手。你的任务是将 AI 编程助手的会话压缩为简洁的摘要。

输出格式：
1. 一句话总结（< 100 字）
2. 关键点列表（3-7 条）
3. 涉及的文件/技术

要求：
- 保留关键决策和解决方案
- 去除重复和冗余
- 保持技术准确性
"#;

fn build_compact_prompt(session: &Session, messages: &[Message]) -> String {
    let mut prompt = format!(
        "项目: {}\n会话时间: {}\n\n",
        session.project_path.as_deref().unwrap_or("unknown"),
        session.updated_at
    );

    for msg in messages {
        let role = if msg.role == "user" { "User" } else { "Assistant" };
        // 截断过长的消息
        let content = truncate(&msg.content, 2000);
        prompt.push_str(&format!("[{}]: {}\n\n", role, content));
    }

    prompt.push_str("\n请生成摘要：");
    prompt
}
```

### 数据存储

#### SQLite 表（三层）

```sql
-- L1: Observations（每个工具调用/操作一个）
CREATE TABLE IF NOT EXISTS observations (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    prompt_number INTEGER NOT NULL,        -- 属于哪一轮对话（从 JSONL user message 顺序推断）
    source_offset INTEGER,                 -- JSONL 行号（用于增量处理）

    -- 结构化字段（部分可直接从工具调用提取，不需要 LLM）
    type TEXT NOT NULL,                    -- bugfix/feature/refactor/change/discovery/decision
    title TEXT NOT NULL,                   -- 短标题
    subtitle TEXT,                         -- 一句话解释
    facts TEXT,                            -- JSON array: 具体事实
    narrative TEXT,                        -- 完整上下文
    files_read TEXT,                       -- JSON array: 直接从工具参数提取
    files_modified TEXT,                   -- JSON array: 直接从工具参数提取

    provider TEXT,                         -- 生成使用的 provider
    model TEXT,
    created_at TEXT NOT NULL,
    FOREIGN KEY (session_id) REFERENCES sessions(id)
);

CREATE INDEX IF NOT EXISTS idx_obs_session ON observations(session_id);
CREATE INDEX IF NOT EXISTS idx_obs_prompt ON observations(session_id, prompt_number);
CREATE INDEX IF NOT EXISTS idx_obs_type ON observations(type);

-- L2: Talk Summary（每轮对话一个）
CREATE TABLE IF NOT EXISTS talk_summaries (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    prompt_number INTEGER NOT NULL,        -- 第几轮对话

    user_request TEXT,                     -- 用户这轮的请求
    summary TEXT NOT NULL,                 -- 这轮对话的摘要
    completed TEXT,                        -- 完成了什么
    files_involved TEXT,                   -- JSON array

    provider TEXT,
    model TEXT,
    tokens_input INTEGER,                  -- 输入 token 数
    tokens_output INTEGER,                 -- 输出 token 数
    created_at TEXT NOT NULL,
    FOREIGN KEY (session_id) REFERENCES sessions(id),
    UNIQUE(session_id, prompt_number)
);

CREATE INDEX IF NOT EXISTS idx_talk_session ON talk_summaries(session_id);
CREATE INDEX IF NOT EXISTS idx_talk_prompt ON talk_summaries(session_id, prompt_number);

-- L3: Session Summary（每个会话一个）
CREATE TABLE IF NOT EXISTS session_summaries (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL UNIQUE,

    summary TEXT NOT NULL,                 -- 一句话总结
    key_points TEXT,                       -- JSON array: 关键点列表
    files_involved TEXT,                   -- JSON array: 涉及的文件
    technologies TEXT,                     -- JSON array: 涉及的技术

    provider TEXT,
    model TEXT,
    tokens_input INTEGER,
    tokens_output INTEGER,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (session_id) REFERENCES sessions(id)
);

CREATE INDEX IF NOT EXISTS idx_session_summary ON session_summaries(session_id);

-- 处理进度追踪
ALTER TABLE sessions ADD COLUMN last_processed_offset INTEGER DEFAULT 0;
ALTER TABLE sessions ADD COLUMN last_processed_prompt INTEGER DEFAULT 0;

-- prompt_number 追踪说明
-- prompt_number 从 JSONL 解析，按 user message 出现顺序编号：
--   第 1 个 type: "user" → prompt_number = 1
--   第 2 个 type: "user" → prompt_number = 2
--   ...
-- 一轮对话 = 一个 user message + 对应的 assistant response
```

#### 向量索引（LanceDB）

复用现有 LanceDB 基建，**L1 + L2 + L3 三层都索引**，用途不同：

| 层级 | 索引 | 用途 |
|------|------|------|
| L1 Observations | ✅ | 精准匹配具体操作（如"那次修 ESLint 配置"） |
| L2 Talk Summary | ✅ | 精准匹配具体对话轮（如"讨论认证方案那轮"） |
| L3 Session Summary | ✅ | 快速定位相关 session（如"处理过登录问题的会话"） |

```rust
// L1 Observation 向量（最细粒度）
struct ObservationVector {
    id: String,
    session_id: String,
    prompt_number: i32,              // 属于哪一轮对话
    observation_type: String,        // bugfix/feature/refactor/...
    title: String,
    embedding: Vec<f32>,             // narrative + facts 拼接后的 embedding
    project_path: String,
    created_at: String,
}

// L2 Talk Summary 向量（每轮对话）
struct TalkSummaryVector {
    id: String,
    session_id: String,
    prompt_number: i32,
    user_request: String,
    summary: String,
    embedding: Vec<f32>,
    project_path: String,
    created_at: String,
}

// L3 Session Summary 向量（整个会话）
struct SessionSummaryVector {
    id: String,
    session_id: String,
    summary: String,
    embedding: Vec<f32>,
    project_path: String,
    created_at: String,
}
```

**搜索路径设计：**
```
快速路径：L3 → 找到相关 session → 再深入 L1/L2 获取细节
精准路径：直接 L1/L2 搜具体操作/对话
```

向量搜索用于智能注入：
1. 用户消息 → embedding
2. 在 L1/L2/L3 向量库中搜索相似摘要
3. 返回 top-k 作为 context

### 配置

```toml
# ~/.config/memex/config.toml

[compact]
# 多层可选开关
l1_observations = false      # L1: Observations（细粒度，类 claude-mem）
l2_talk_summary = false      # L2: Talk Summary（每轮对话）
l3_session_summary = true    # L3: Session Summary（自动开启 l2）

# L1 优化选项
[compact.l1]
prune_empty = true           # 跳过空输出的工具调用
merge_consecutive = true     # 合并连续同类操作
merge_threshold = 3          # 连续 N 个同类操作合并为 1 个

# Provider 配置
provider = "ollama"          # ollama | openai | openrouter | anthropic

[compact.ollama]
endpoint = "http://localhost:11434"
model = "qwen3:8b"

[compact.openai]
endpoint = "https://api.openai.com/v1"
api_key = "${OPENAI_API_KEY}"
model = "gpt-4o-mini"

[compact.openrouter]
api_key = "${OPENROUTER_API_KEY}"
model = "xiaomi/mimo-v2-flash:free"  # 免费模型

[compact.anthropic]
api_key = "${ANTHROPIC_API_KEY}"
model = "claude-3-haiku-20240307"
```

#### 配置校验逻辑

```rust
// L3 依赖 L2，自动开启
if config.l3_session_summary && !config.l2_talk_summary {
    config.l2_talk_summary = true;
    log::info!("L3 需要 L2，已自动开启 L2");
}
```

#### 用户选择示例

| 选择 | L1 | L2 | L3 | 场景 |
|------|----|----|----|----|
| 极简 | ❌ | ❌ | ❌ | 只用原文搜索 |
| 快速预览 | ❌ | ✅ | ✅ | 只要 Session 摘要 |
| 完整（类 mem） | ✅ | ✅ | ✅ | 社区兼容 |
| 细粒度搜索 | ✅ | ❌ | ❌ | 只要 Observations |

环境变量：
```bash
MEMEX_COMPACT_PROVIDER=ollama
OPENAI_API_KEY=sk-xxx
OPENROUTER_API_KEY=sk-or-xxx
ANTHROPIC_API_KEY=sk-ant-xxx
```

## 数据来源架构

Memex 支持两条**实时**数据来源路径：

```
┌─────────────────────────────────────────────────────────────┐
│                    两条实时路径                              │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  路径 1：memex-rs ← JSONL 文件监听 (实时，通用)               │
│  ├── watcher.rs 监听文件变化                                 │
│  ├── 新内容 → 解析 → 生成 L1/L2                              │
│  └── prompt_number 从 user message 顺序推断                 │
│                                                             │
│  路径 2：MemexKit ← ClaudeKit hooks (实时，ETerm 特有)        │
│  └── hooks event 直接触发                                    │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

| | memex-rs (JSONL 监听) | MemexKit (hooks) |
|---|---|---|
| 触发方式 | JSONL 文件变化 | Claude hooks event |
| 适用范围 | 通用（任何 Claude Code 用户） | ETerm 特有 |
| prompt_number | 从 JSONL 解析计数 | 从 hooks 上下文获取 |

### 数据流架构

```
┌─────────────────────────────────────────────────────────────────────┐
│  ai-cli-session-collector (Adapter 层)                              │
│  ├── ConversationAdapter trait                                      │
│  ├── ClaudeAdapter: JSONL → ParsedMessage                          │
│  ├── CodexAdapter: JSONL → ParsedMessage                           │
│  └── AdapterRegistry: 自动发现所有 adapter                          │
└─────────────────────────────────┬───────────────────────────────────┘
                                  │ 统一的 ParsedMessage
                                  ▼
┌─────────────────────────────────────────────────────────────────────┐
│  memex-rs (Collector 层)                                            │
│  └── 存入 SQLite (sessions/messages 表)                             │
└─────────────────────────────────┬───────────────────────────────────┘
                                  │ DbMessage
                                  ▼
┌─────────────────────────────────────────────────────────────────────┐
│  memex-rs/compact (Compact 层)                                      │
│  ├── build_parsed_session(): DbMessage → ParsedSession/Talk         │
│  └── 生成 L1/L2/L3 摘要                                             │
└─────────────────────────────────────────────────────────────────────┘
```

**关键设计**：Compact 层操作的是数据库的统一 `DbMessage`，不关心原始 CLI 格式。
新增 CLI 支持只需在 `ai-cli-session-collector` 添加 adapter，Compact 层无需修改。

## Lifecycle 设计

### 触发时机（Compact 生成）

#### L1/L2 生成：实时（文件监听）

```rust
// watcher.rs 已有文件监听
// JSONL 文件更新 → 解析新内容 → 增量生成 L1/L2

async fn handle_jsonl_update(&self, session_id: &str, new_content: &str) {
    // 解析新的 messages/tool_calls
    // 更新 prompt_number
    // 生成 L1 observations
    // 生成 L2 talk_summary（当一轮对话完整时）
}
```

#### L3 生成：Session 结束后

```
Phase 1（先做）：
├── Session idle 检测（如 5 分钟无新写入）
├── Stop hook 触发
└── 闪退重启后收敛补生成

Phase 2（以后）：
├── 每 N 轮增量更新
└── N + M（stop）场景优化
```

```rust
async fn handle_session_idle(&self, session_id: &str) {
    // 检查是否已有 L3
    // 从 L2 汇总生成 L3
}
```

**辅助方式**：

1. **Hook 触发**（社区用户可选）
   ```json
   // ~/.claude/settings.json
   {
     "hooks": {
       "Stop": [{
         "commands": ["curl -X POST http://localhost:10013/api/compact/session/$SESSION_ID"]
       }]
     }
   }
   ```

2. **定时任务**
   - 每小时处理未 compact 的 session
   - 可配置延迟（如会话结束 1 小时后再 compact）

3. **手动触发**
   ```bash
   curl -X POST http://localhost:10013/api/compact/trigger
   curl -X POST http://localhost:10013/api/compact/session/{session_id}
   ```

### 注入策略（Compact 消费）

三层设计，满足不同用户需求：

| 层级 | 方式 | 特点 | 适用用户 |
|------|------|------|----------|
| **L1** | 全量注入 (Push) | SessionStart 时注入所有历史摘要 | 社区/claude-mem 迁移用户 |
| **L2** | 主动查询 (Pull) | Claude 通过 MCP 主动调用查询 | 普通用户 |
| **L3** | 向量匹配 (Smart) | 根据当前消息匹配相关历史 | 高级用户（推荐） |

#### L1: 全量注入（社区兼容 Feature）

```json
// ~/.claude/settings.json
{
  "hooks": {
    "SessionStart": [{
      "commands": ["memex-inject --mode=full"]
    }]
  }
}
```

- 默认关闭
- 兼容 claude-mem 用户迁移
- 简单粗暴，Token 消耗大

#### L2: 主动查询（MCP Pull）

Claude 通过 CLAUDE.md 知道有 memex MCP 可用：

```markdown
# ~/.claude/CLAUDE.md

## Memex 历史查询
当你需要参考历史上下文时，可以调用 memex MCP：
- search_history: 搜索相关历史
- get_session: 获取会话详情
- get_recent_sessions: 最近会话
```

Claude 根据需要主动查询，不污染每个会话。

#### L3: 向量匹配 Push 注入（Alpha 功能）

> ⚠️ **Alpha 阶段**：此功能需要长期测试迭代，包括置信度阈值、top-k 选择、Token 消耗等参数调优。

```
用户发消息
       ↓
  UserPromptSubmit Hook 触发
       ↓
  消息做 embedding（EmbeddingProvider）
       ↓
  在 L1/L2 向量库中搜索相似历史（LanceDB）
       ↓
  找到 top-k 相关历史摘要
       ↓
  注入相关 context（置信度 + 摘要）
```

**注入内容格式**：
```rust
struct InjectedMemory {
    session_id: String,
    prompt_number: Option<i32>,
    summary: String,
    confidence: f32,          // 置信度分数
    // 提示 Claude 可以调用 MCP 获取更多
    hint: String,             // "Call get_session({session_id}) for full context"
}
```

**实现**：

```json
// ~/.claude/settings.json
{
  "hooks": {
    "UserPromptSubmit": [{
      "commands": ["memex-inject --mode=smart --query=\"$PROMPT\""]
    }]
  }
}
```

**待迭代参数**（Alpha 阶段确定）：
- `similarity_threshold`: 相似度阈值（默认 0.7？）
- `max_results`: 最大返回数（默认 5？）
- `max_tokens`: 注入内容最大 token（默认 2000？）
- `project_scope`: 只搜当前项目？还是跨项目？

**测试计划**：
- 使用历史 user prompt 作为测试集
- 评估匹配质量和 Token 消耗
- 长期迭代参数

**优势**：
- 精准注入，不污染无关会话
- Token 高效（只注入相关内容）
- 自动发现关联，不遗漏
- **这是 Memex 相对 claude-mem 的核心差异化**

### API 设计

```
POST /api/compact/trigger
    → 触发增量 compact（处理未压缩的 session）

POST /api/compact/session/{session_id}
    → 对指定 session 进行 compact

GET /api/compact/status
    → 返回 compact 状态（已处理/待处理/失败）

GET /api/sessions/{session_id}/summary
    → 返回 session 的压缩摘要（如果有）
```

### MCP 集成

#### 搜索级别参数

四级搜索粒度，通过 `level` 参数选择：

```rust
/// 搜索级别
pub enum SearchLevel {
    Raw,          // L0 原文（默认，现有行为）
    Observations, // L1 工具调用级别
    Talks,        // L2 对话轮级别
    Sessions,     // L3 会话级别
    All,          // 全部级别搜索，合并结果
}

/// search_history 参数扩展
pub struct SearchParams {
    pub query: String,
    pub level: Option<SearchLevel>,  // 新增，默认 Raw
    pub limit: Option<usize>,
    pub project: Option<String>,
    // ... 其他现有参数
}
```

**使用示例：**
```
search_history(query="认证")                   → 默认搜原文（兼容现有行为）
search_history(query="认证", level="raw")      → 搜 L0 原文
search_history(query="认证", level="observations") → 搜 L1 Observations
search_history(query="认证", level="talks")    → 搜 L2 Talk Summary
search_history(query="认证", level="sessions") → 搜 L3 Session Summary
search_history(query="认证", level="all")      → 全部级别搜索，合并排序
```

**设计原则：**
- 默认行为不变（level=raw），向后兼容
- 用户按需选择粒度
- `level=all` 适合探索性搜索

#### 返回结果扩展

```rust
pub struct SearchResult {
    pub session_id: String,
    pub message_id: Option<String>,      // L0 有，L1/L2/L3 可能没有
    pub prompt_number: Option<i32>,      // L1/L2 有
    pub level: SearchLevel,              // 新增：结果来自哪个级别
    pub content: String,                 // 原文或摘要内容
    pub score: f32,
}

// get_session 可选返回摘要而非原文
// GET /api/sessions/{id}?format=summary
```

## 模块结构

```
memex-rs/src/
├── llm/                     # LLM 抽象层 ✅ 已完成
│   ├── mod.rs               # 模块入口，re-export
│   ├── core.rs              # LlmClientCore（共享 HTTP client）
│   ├── types.rs             # ChatMessage, ChatRole, ChatResponse
│   ├── chat.rs              # ChatProvider trait + ChatProviderExt
│   ├── embedding.rs         # EmbeddingProvider trait
│   └── providers/
│       ├── ollama.rs        # Ollama 实现
│       └── openai.rs        # OpenAI 兼容实现（含 OpenRouter）
│
├── compact/                 # Compact 层 ✅ 已完成
│   ├── mod.rs               # 模块入口
│   ├── config.rs            # 多层开关配置（L1/L2/L3 开关）
│   ├── db.rs                # SQLite 存储（observations/talk_summaries/session_summaries）
│   ├── service.rs           # 核心服务（L1/L2/L3 生成逻辑）
│   ├── source.rs            # 数据源解析（ParsedSession/Talk/ToolCall）
│   ├── prompt.rs            # Prompt 模板（L1/L2/L3）
│   ├── queue.rs             # 异步队列 + SessionTracker（idle 检测）
│   ├── vector.rs            # LanceDB 向量存储（L1/L2/L3）
│   └── indexer.rs           # 向量索引器
│
├── search/                  # 混合检索 ✅ 已完成
│   └── mod.rs               # FTS + Vector + RRF 融合，多级别搜索
│
├── embedding/               # 保留兼容
│   └── mod.rs
│
└── rag/
    └── mod.rs
```

## 实现计划

### Phase 0: LLM 层重构（前置依赖）✅ 已完成
- [x] 创建 `llm/mod.rs`，定义 ChatProvider + EmbeddingProvider traits
- [x] 实现 OllamaProvider
- [x] 实现 OpenAIProvider（含 OpenRouter、DeepSeek）
- [x] ChatProviderExt 扩展方法

### Phase 1: 多层压缩基础框架 ✅ 已完成
- [x] 添加 observations / talk_summaries / session_summaries 表 migration (`compact/db.rs`)
- [x] 实现 prompt_number 解析（从 JSONL user message 顺序推断）(`compact/source.rs`)
- [x] 实现 L1 ObservationService (`compact/service.rs`)
  - [x] 从 JSONL 提取工具调用
  - [x] 剪枝：跳过空输出
  - [x] 合并：连续同类操作
  - [x] 结构化提取（files 直接从参数拿）
  - [x] LLM 生成 type/title/summary
- [x] 实现 L2 TalkSummaryService (`compact/service.rs`)
  - [x] 从原文生成每轮 Summary
- [x] 实现 L3 SessionSummaryService (`compact/service.rs`)
  - [x] Session idle 检测触发 (`compact/queue.rs` SessionTracker)
  - [x] 从 L2 汇总生成
  - [x] 闪退重启后收敛补生成
- [x] 增量处理（prompt_number cursor 追踪）(`compact/db.rs` ProcessingProgress)
- [x] watcher.rs 集成（JSONL 文件监听触发 CompactQueue）
- [x] 数据组织结构（`compact/source.rs` ParsedSession/Talk/ToolCall + build_parsed_session）

### Phase 2: 向量索引 + 搜索级别 ✅ 已完成
- [x] L1 observation_vectors LanceDB 表 (`compact/vector.rs` CompactVectorStore)
- [x] L2 talk_summary_vectors LanceDB 表
- [x] L3 session_summary_vectors LanceDB 表
- [x] 摘要 embedding 自动索引 (`compact/indexer.rs` CompactIndexer)
- [x] 相似度搜索 API
- [x] **search_history level 参数** (`search/mod.rs`)
  - [x] SearchLevel 枚举（Raw/Observations/Talks/Sessions/All）
  - [x] 各级别搜索实现（FTS + Vector + Hybrid）
  - [x] 结果合并排序（level=all）
  - [x] 向后兼容（默认 level=raw）
  - [x] project_id / date 过滤支持

### Phase 3: 注入策略
- [ ] Pull 模式：MCP 主动查询（已有基础）
- [ ] Push 模式 - 全量：SessionStart Hook 全量注入（社区兼容）
- [ ] Push 模式 - Smart：UserPromptSubmit Hook 向量匹配注入（**Alpha**）
  - [ ] 置信度计算
  - [ ] 注入格式设计
  - [ ] 参数调优（threshold, top-k, max_tokens）
  - [ ] 用历史 user prompt 测试效果
- [ ] 配置文件支持（选择注入策略）

### Phase 4: 多 Provider + 优化
- [ ] OpenRouter provider（免费模型支持）
- [ ] Anthropic provider
- [ ] Provider fallback 机制
- [ ] 统计（token 节省量、命中率）
- [ ] Prompt 优化（多语言支持）

### Phase 5: 多 Adapter 扩展 ✅ 已完成（在 ai-cli-session-collector）
- [x] Adapter 架构已实现（`AdapterRegistry` + `ConversationAdapter` trait）
- [x] Claude adapter
- [x] Codex adapter
- [x] Gemini adapter
- [x] AI CLI Kit（ETerm 集成）

> **注**：Adapter 负责解析不同 CLI 的 JSONL 格式，实现在 `ai-cli-session-collector` crate。
> Compact 层直接操作数据库的统一 Message，不关心原始格式。

## 设计决策

| 问题 | 决策 | 理由 |
|------|------|------|
| 原文保留 | **始终保留** | Memex 的护城河，压缩层可重做 |
| 压缩层级 | 三层可选（L1/L2/L3） | 用户按需选择，灵活度高 |
| L3 依赖 | 必须开启 L2 | 避免原文过长超出 context 限制 |
| L1 生成 | 从原文，剪枝合并 | 单个工具调用不长，可优化数量 |
| L2 生成 | 从原文 | 一轮对话 token 可控 |
| L3 生成 | 从 L2 汇总 | 多轮对话原文太长 |
| L3 生成时机 | Session 结束后 | 先简单实现，后续支持每 N 轮增量 |
| 向量索引 | **L1 + L2 + L3 都索引** | L1/L2 精准搜索，L3 快速定位 |
| prompt_number | 从 JSONL 解析 | 按 user message 顺序编号 |
| 数据来源 | 两条实时路径 | JSONL 监听（通用）+ hooks（ETerm） |
| 增量更新 | 支持（prompt_number cursor） | 不重复处理已有内容 |
| LLM 抽象 | 复用统一 trait | 不为 Compact 单独定义 |
| 注入方式 | **Push + Pull** | Push Smart 是核心差异化 |
| Push Smart | Alpha 功能 | 需要长期测试迭代参数 |
| 搜索级别 | 四级可选（L0/L1/L2/L3） | 默认 L0 原文，向后兼容 |
| 多 Adapter | 先 Claude，架构预留 | Scope 可控，后续扩展 |

## 开放问题

1. **多语言 Prompt**：是否需要根据会话语言自动切换 Prompt？
2. **项目级 vs 全局**：向量搜索是否限定在当前项目？还是跨项目？（Alpha 阶段确定）
3. **免费模型质量**：OpenRouter 免费模型（如 MiMo）摘要质量是否足够？
4. **Hook 脚本分发**：memex-inject 脚本如何安装和更新？
5. **Push Smart 参数**：置信度阈值、top-k、max_tokens 需要实际测试确定

## 参考

- [claude-mem](https://github.com/thedotmack/claude-mem) - Claude SDK compact 实现（调研对象）
- [claude-user-memory](https://github.com/VAMFI/claude-user-memory) - Prompt 框架，置信度公式（调研对象）
- [Anthropic API](https://docs.anthropic.com/claude/reference/messages_post)
- [OpenAI API](https://platform.openai.com/docs/api-reference/chat)
- [OpenRouter](https://openrouter.ai/docs) - 多模型网关，含免费模型
