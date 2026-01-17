# RFC-002: Memex Lite 版本

> 无数据库依赖的轻量级 CLI 工具，直接读取各 AI CLI 原始数据

## 背景

Memex 完整版需要运行 daemon 服务，依赖 SQLite + LanceDB + Ollama。对于以下场景，这过于重量级：

1. **快速搜索**：只想临时搜一下历史，不想启动服务
2. **跨机器**：在新机器上快速查看历史，不想配置完整环境
3. **CI/CD**：在自动化流程中查询历史
4. **资源受限**：低配机器、容器环境

## 目标

1. **零依赖**：单 binary，不需要数据库
2. **多数据源**：支持 Claude Code / Codex / OpenCode / Gemini CLI 等
3. **即时可用**：直接读取原始 JSONL/JSON 文件
4. **跨 CLI 共享**：统一搜索所有 AI CLI 的历史

## 与完整版的关系

```
┌───────────────────────────────────────────────────────────────────┐
│                  ai-cli-session-collector                          │
│  ┌───────────────────────────────────────┐                        │
│  │            Adapter 层 (核心解析)       │  ← 轻量依赖            │
│  │  Claude │ Codex │ OpenCode │ Gemini   │     serde/dirs/chrono  │
│  └───────────────────────────────────────┘                        │
└───────────────────────────────────────────────────────────────────┘
                       │
         ┌─────────────┴─────────────┐
         │                           │
         ↓                           ↓
┌─────────────────────────┐  ┌─────────────────────────┐
│   claude-session-db     │  │      Memex Lite         │
│  ┌─────────────────┐    │  │                         │
│  │ SQLite + FTS5   │    │  │  ┌─────────────────┐   │
│  │ Writer 协调      │    │  │  │  Grep Search    │   │
│  └─────────────────┘    │  │  └─────────────────┘   │
└────────────┬────────────┘  │  ┌─────────────────┐   │
             │               │  │     CLI         │   │
             ↓               │  └─────────────────┘   │
┌─────────────────────────┐  └─────────────────────────┘
│      Memex 完整版        │
│  ┌─────────┐ ┌────────┐ │
│  │ LanceDB │ │ Ollama │ │
│  │ 向量搜索 │ │Embedding│ │
│  └─────────┘ └────────┘ │
└─────────────────────────┘
```

**关键点**：Lite 直接依赖 `ai-cli-session-collector`，不经过 `claude-session-db`，避免引入重依赖。

## 设计

### 数据源发现

```rust
// memex-lite/src/source.rs

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CliType {
    Claude,
    Codex,
    OpenCode,
    Gemini,
    // 可扩展
}

pub struct DataSource {
    pub cli: CliType,
    pub name: &'static str,
    pub base_path: PathBuf,
    pub pattern: &'static str,  // glob pattern
}

impl DataSource {
    /// 自动发现所有可用的数据源
    pub fn discover_all() -> Vec<DataSource> {
        let home = dirs::home_dir().unwrap();
        let mut sources = Vec::new();

        // Claude Code
        let claude_path = home.join(".claude/projects");
        if claude_path.exists() {
            sources.push(DataSource {
                cli: CliType::Claude,
                name: "Claude Code",
                base_path: claude_path,
                pattern: "**/*.jsonl",
            });
        }

        // Codex CLI
        let codex_path = home.join(".codex");
        if codex_path.exists() {
            sources.push(DataSource {
                cli: CliType::Codex,
                name: "Codex CLI",
                base_path: codex_path,
                pattern: "**/*.jsonl",
            });
        }

        // OpenCode
        let opencode_path = home.join(".opencode");
        if opencode_path.exists() {
            sources.push(DataSource {
                cli: CliType::OpenCode,
                name: "OpenCode",
                base_path: opencode_path,
                pattern: "**/*.json",
            });
        }

        // Gemini CLI (假设路径)
        let gemini_path = home.join(".gemini");
        if gemini_path.exists() {
            sources.push(DataSource {
                cli: CliType::Gemini,
                name: "Gemini CLI",
                base_path: gemini_path,
                pattern: "**/*.jsonl",
            });
        }

        sources
    }
}
```

### Adapter 复用

直接依赖 `ai-cli-session-collector`（核心解析层，轻量依赖）：

```rust
// memex-lite/src/adapter.rs

// 直接使用 ai-cli-session-collector
pub use ai_cli_session_collector::{
    ClaudeAdapter,
    CodexAdapter,
    OpenCodeAdapter,
    ConversationAdapter,
    all_adapters,
    ParsedMessage,
    IndexableSession,
};
```

> 注：不从 `memex-rs` 或 `claude-session-db` 复用，因为它们会带入 rusqlite/tokio/lancedb 等重依赖。

### 搜索实现

```rust
// memex-lite/src/search.rs

use grep_regex::RegexMatcher;
use grep_searcher::{Searcher, SearcherBuilder, Sink, SinkMatch};
use std::path::Path;

pub struct SearchOptions {
    pub query: String,
    pub case_insensitive: bool,
    pub max_results: usize,
    pub cli_filter: Option<Vec<CliType>>,
    pub project_filter: Option<String>,
}

pub struct SearchResult {
    pub cli: CliType,
    pub session_id: String,
    pub project: Option<String>,
    pub file_path: PathBuf,
    pub line_number: usize,
    pub content: String,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
}

pub struct GrepSearcher {
    sources: Vec<DataSource>,
}

impl GrepSearcher {
    pub fn new() -> Self {
        Self {
            sources: DataSource::discover_all(),
        }
    }

    /// 搜索所有数据源
    pub fn search(&self, options: &SearchOptions) -> anyhow::Result<Vec<SearchResult>> {
        let matcher = RegexMatcher::new(&options.query)?;
        let mut results = Vec::new();

        for source in &self.sources {
            // 跳过不需要的 CLI
            if let Some(ref filter) = options.cli_filter {
                if !filter.contains(&source.cli) {
                    continue;
                }
            }

            // 遍历该数据源的所有文件
            for entry in glob::glob(&format!("{}/{}", source.base_path.display(), source.pattern))? {
                let path = entry?;

                // 项目过滤
                if let Some(ref project) = options.project_filter {
                    if !path.to_string_lossy().contains(project) {
                        continue;
                    }
                }

                // 搜索文件
                let file_results = self.search_file(&path, &matcher, source.cli)?;
                results.extend(file_results);

                if results.len() >= options.max_results {
                    break;
                }
            }
        }

        Ok(results)
    }

    fn search_file(
        &self,
        path: &Path,
        matcher: &RegexMatcher,
        cli: CliType,
    ) -> anyhow::Result<Vec<SearchResult>> {
        // 使用 ripgrep 的 grep-searcher 库
        // ...
    }
}
```

### CLI 设计

```rust
// memex-lite/src/main.rs

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "memex-lite")]
#[command(about = "Lightweight AI CLI history search tool")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Search across all AI CLI histories
    Search {
        /// Search query (regex supported)
        query: String,

        /// Filter by CLI type (claude, codex, opencode, gemini)
        #[arg(short, long)]
        cli: Option<Vec<String>>,

        /// Filter by project name/path
        #[arg(short, long)]
        project: Option<String>,

        /// Maximum results
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,

        /// Case insensitive search
        #[arg(short, long)]
        ignore_case: bool,

        /// Show context lines
        #[arg(short = 'C', long, default_value = "2")]
        context: usize,

        /// Output format (text, json)
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// List all sessions
    List {
        /// Filter by CLI type
        #[arg(short, long)]
        cli: Option<Vec<String>>,

        /// Filter by project
        #[arg(short, long)]
        project: Option<String>,

        /// Show recent N sessions
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,

        /// Sort by (time, size)
        #[arg(short, long, default_value = "time")]
        sort: String,
    },

    /// View a specific session
    View {
        /// Session ID (or file path)
        session: String,

        /// Show only user messages
        #[arg(long)]
        user_only: bool,

        /// Show only assistant messages
        #[arg(long)]
        assistant_only: bool,

        /// Output format (text, json, markdown)
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// Show statistics
    Stats {
        /// Show per-CLI breakdown
        #[arg(short, long)]
        by_cli: bool,

        /// Show per-project breakdown
        #[arg(short, long)]
        by_project: bool,
    },

    /// Export session(s) to file
    Export {
        /// Session ID or "all"
        session: String,

        /// Output file/directory
        #[arg(short, long)]
        output: PathBuf,

        /// Export format (jsonl, json, markdown)
        #[arg(short, long, default_value = "jsonl")]
        format: String,
    },

    /// Show available data sources
    Sources,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Search { query, cli, project, limit, ignore_case, context, format } => {
            cmd_search(query, cli, project, limit, ignore_case, context, format)
        }
        Commands::List { cli, project, limit, sort } => {
            cmd_list(cli, project, limit, sort)
        }
        Commands::View { session, user_only, assistant_only, format } => {
            cmd_view(session, user_only, assistant_only, format)
        }
        Commands::Stats { by_cli, by_project } => {
            cmd_stats(by_cli, by_project)
        }
        Commands::Export { session, output, format } => {
            cmd_export(session, output, format)
        }
        Commands::Sources => {
            cmd_sources()
        }
    }
}
```

### 使用示例

```bash
# 安装
cargo install memex-lite
# 或
brew install memex-lite

# 搜索所有 CLI 历史
memex-lite search "API 设计"

# 只搜索 Claude Code
memex-lite search "hook" --cli claude

# 搜索指定项目
memex-lite search "bug" --project myapp

# 忽略大小写 + 显示上下文
memex-lite search "error" -i -C 5

# JSON 输出（便于管道处理）
memex-lite search "TODO" --format json | jq '.[] | .content'

# 列出最近 10 个会话
memex-lite list -n 10

# 列出某项目的会话
memex-lite list --project myapp

# 查看会话
memex-lite view abc123-def456

# 只看用户消息
memex-lite view abc123 --user-only

# 导出为 Markdown
memex-lite view abc123 --format markdown > session.md

# 统计
memex-lite stats
memex-lite stats --by-cli
memex-lite stats --by-project

# 显示可用数据源
memex-lite sources
```

### 输出示例

```
$ memex-lite search "API 设计"

[Claude Code] myapp (2 matches)
───────────────────────────────────────
Session: abc123-def456-7890
Time: 2025-01-15 14:30

  [User] 帮我设计一个 API 接口
  [Assistant] 好的，我来帮你设计 API...
         ↑ match at line 42

Session: def789-abc123-4567
Time: 2025-01-14 10:15

  [User] 这个 API 设计有什么问题？
         ↑ match at line 15

[Codex] backend-service (1 match)
───────────────────────────────────────
Session: xyz-123
Time: 2025-01-13 09:00

  [User] 参考之前的 API 设计
         ↑ match at line 8

Found 3 matches across 2 CLIs
```

## 模块结构

```
memex-lite/
├── Cargo.toml
├── src/
│   ├── main.rs          # CLI 入口
│   ├── source.rs        # 数据源发现
│   ├── adapter.rs       # 复用 memex adapter
│   ├── search.rs        # grep 搜索
│   ├── list.rs          # 会话列表
│   ├── view.rs          # 会话查看
│   ├── stats.rs         # 统计
│   ├── export.rs        # 导出
│   └── output.rs        # 输出格式化
└── tests/
    └── integration.rs
```

## Cargo.toml

```toml
[package]
name = "memex-lite"
version = "0.1.0"
edition = "2021"
description = "Lightweight AI CLI history search tool"
license = "MIT"

[dependencies]
# CLI
clap = { version = "4", features = ["derive"] }
colored = "2"

# 搜索
grep-regex = "0.1"
grep-searcher = "0.1"
glob = "0.3"
regex = "1"

# 数据解析 (直接依赖核心解析层，轻量)
ai-cli-session-collector = { git = "https://github.com/vimo-ai/ai-cli-session-collector" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# 工具
anyhow = "1"
dirs = "5"
chrono = "0.4"

[features]
default = []
```

## 实现计划

### Phase 1: 核心功能
- [ ] 数据源发现 (Claude Code 优先)
- [ ] 集成 ai-cli-session-collector adapter
- [ ] grep 搜索实现
- [ ] CLI 基础命令 (search, list, view)

### Phase 2: 多 CLI 支持
- [ ] Codex adapter
- [ ] OpenCode adapter
- [ ] Gemini adapter
- [ ] CLI 过滤选项

### Phase 3: 增强功能
- [ ] 统计功能
- [ ] 导出功能
- [ ] JSON 输出格式
- [ ] 性能优化（并行搜索）

### Phase 4: 发布
- [ ] cargo publish
- [ ] Homebrew formula
- [ ] GitHub releases (预编译 binary)

## 与完整版的区别

| 特性 | memex 完整版 | memex-lite |
|------|-------------|------------|
| 依赖 | SQLite + LanceDB + Ollama | 无 |
| 部署 | daemon 服务 | 单 binary CLI |
| 30天备份 | ✅ | ❌ |
| FTS 搜索 | ✅ | ❌ (grep) |
| 向量搜索 | ✅ | ❌ |
| Compact 摘要 | ✅ | ❌ |
| MCP Server | ✅ | ❌ |
| Web UI | ✅ | ❌ |
| 跨 CLI 共享 | ✅ | ✅ |
| 即时使用 | ❌ (需启动) | ✅ |
| 资源占用 | 高 | 极低 |

## 开放问题

1. **是否支持 MCP？**
   - 可以考虑简化版 MCP server（stdio 模式）
   - 但这会增加复杂度

2. **缓存？**
   - 可选的会话元数据缓存（`~/.cache/memex-lite/`）
   - 加速 list 命令

3. **配置文件？**
   - `~/.config/memex-lite/config.toml`
   - 自定义数据源路径
