# ai-cli-session-collector 设计文档

> 从 memex-rs 提取通用 AI CLI 会话收集层，供 memex、vlaude 及未来产品共享。
>
> **仓库**: https://github.com/vimo-ai/AI-cli-session-collector

## 背景

### 当前状态

```
memex-rs (Rust)          vlaude-daemon (Node.js)
├── adapter/             ├── shared-core/
│   ├── claude.rs        │   ├── jsonl-parser
│   └── codex.rs         │   └── path-codec
└── 业务逻辑...          └── 业务逻辑...

问题：两套代码，逻辑重复
```

### 目标状态

```
ai-cli-session-collector (Rust)
├── 通用 AI CLI 会话收集
├── Claude / Codex / Gemini / Cursor ...
└── 纯解析，无 IO 依赖
         ↑                    ↑
         │                    │
      memex                vlaude
   (索引、向量)          (远程控制)
```

## 产品独立性

```
用户 A: 只装 memex      → 会话索引、搜索、RAG
用户 B: 只装 vlaude     → 远程控制、跨设备
用户 C: 都装            → 完整体验
ETerm:  内置插件        → 更好的集成
```

**关键**：memex 和 vlaude 是独立产品，ai-cli-session-collector 是共享基础。

## 架构设计

### 分层

```
┌─────────────────────────────────────────────────────────────────┐
│                ai-cli-session-collector                          │
│                                                                 │
│  adapter/                         domain/                       │
│  ├── ConversationAdapter (trait)  ├── Source (enum)             │
│  ├── ClaudeAdapter                ├── MessageType (enum)        │
│  ├── CodexAdapter                 ├── SessionMeta               │
│  └── (未来更多)                   ├── ParsedMessage             │
│                                   └── ParseResult               │
│                                                                 │
│  无 SQLite、无 HTTP、无向量 —— 纯收集/解析逻辑                    │
└─────────────────────────────────────────────────────────────────┘
                    ↑                         ↑
        ┌───────────┴───────────┐   ┌────────┴────────┐
        │        memex          │   │   vlaude-core   │
        │  + SQLite/FTS5        │   │  + Server 通信  │
        │  + Ollama/LanceDB     │   │  + Session 同步 │
        │  + MCP Server         │   │  + 远程注入     │
        └───────────────────────┘   └─────────────────┘
                    │                         │
        ┌───────────┴───────────┐   ┌────────┴────────┐
        │  memex (bin + dylib)  │   │ vlaude (bin+dylib)
        │                       │   │                 │
        │  独立: brew install   │   │  独立运行       │
        │  ETerm: MemexKit      │   │  ETerm: VlaudeKit
        └───────────────────────┘   └─────────────────┘
```

### ConversationAdapter Trait

```rust
pub trait ConversationAdapter: Send + Sync {
    /// 数据来源标识
    fn source(&self) -> Source;

    /// 列出所有会话元数据
    fn list_sessions(&self) -> Result<Vec<SessionMeta>>;

    /// 解析单个会话
    fn parse_session(&self, meta: &SessionMeta) -> Result<Option<ParseResult>>;
}
```

### 已支持的适配器

| Adapter | 数据源 | 路径 |
|---------|--------|------|
| ClaudeAdapter | Claude Code | `~/.claude/projects/{encoded}/{session}.jsonl` |
| CodexAdapter | Codex CLI | `~/.codex/history.jsonl` + `~/.codex/sessions/` |

### 未来可扩展

- GeminiAdapter (Gemini CLI)
- CursorAdapter (Cursor)
- WindsurfAdapter
- ...

## 目录结构

```
ai-cli-session-collector/
├── Cargo.toml
├── README.md
└── src/
    ├── lib.rs
    ├── adapter/
    │   ├── mod.rs
    │   ├── claude.rs      ← ClaudeAdapter
    │   └── codex.rs       ← CodexAdapter
    └── domain/
        ├── mod.rs
        └── types.rs       ← Source, MessageType, SessionMeta, ParsedMessage, ParseResult
```

## 实施步骤 (已完成 ✅)

1. ✅ 创建 ai-cli-session-collector crate
2. ✅ 提取 adapter 和 domain 类型
3. ✅ 最小化依赖
4. ✅ memex-rs 改为 git 依赖
5. ✅ 验证通过

```toml
# memex-rs/Cargo.toml
[dependencies]
ai-cli-session-collector = { git = "https://github.com/vimo-ai/AI-cli-session-collector" }
```

## 后续工作

### Phase 2: memex 双产物

memex-rs 同时输出 binary 和 dylib：

```toml
[lib]
crate-type = ["cdylib", "rlib"]

[[bin]]
name = "memex"
```

### Phase 3: MemexKit (ETerm 插件)

加载 libmemex.dylib，提供 UI 和 MCP 集成。

### Phase 4: vlaude-core

从 vlaude-daemon 提取核心逻辑，依赖 ai-cli-session-collector。

### Phase 5: VlaudeKit (ETerm 插件)

直连 server，替代 daemon 的 EtermGateway。

## 相关文件

- 源代码: `/Users/higuaifan/Desktop/vimo/memex/memex-rs/src/adapter/`
- memex 设计: `/Users/higuaifan/Desktop/vimo/memex/DESIGN.md`
- vlaude daemon: `/Users/higuaifan/Desktop/hi/小工具/claude/packages/vlaude-daemon/`
- ETerm 插件系统: `/Users/higuaifan/Desktop/hi/小工具/english/Plugins/`

## 注意事项

1. **保持向后兼容** - memex-rs 的 API 不变
2. **最小依赖** - ai-session-core 只做解析
3. **测试覆盖** - 提取后确保现有测试通过
4. **渐进式** - 一步步来，每步可验证
