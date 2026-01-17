<p align="center">
  <img src=".github/assets/logo.svg" width="300" alt="Memex Logo">
</p>

<h1 align="center">Memex</h1>

<p align="center">
  <strong>One Memory. All CLIs. Never Compacted. Exact Search.</strong>
</p>

<p align="center">
  <a href="README.md">English</a> | <a href="/.github/README_zh-CN.md">中文</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-1.75+-red" alt="Rust">
  <img src="https://img.shields.io/badge/license-MIT-green" alt="License">
</p>

---

Session history management for AI coding assistants. Never lose your conversations again.

## Supported Tools

- ✅ Claude Code
- ✅ Codex CLI
- ✅ OpenCode
- ✅ Gemini CLI

## Why Memex?

AI CLI tools' local conversation data often expires or gets lost, causing:
- Loss of important technical decision records
- Difficulty searching historical conversations
- Knowledge cannot be accumulated and reused

Memex solves these problems:
- Automatic backup of all AI CLI sessions
- Powerful full-text and semantic search
- MCP protocol support for searching history directly in Claude
- REST API for integration

## Features

### Data Collection & Backup
- Automatically scans all sessions under `~/.claude/projects/`
- Parses JSONL format conversation content
- Stores in SQLite database with FTS5 full-text index
- Daily incremental backups

### Search Capabilities
- **Full-text Search**: Fast keyword search based on SQLite FTS5
- **Semantic Search**: Vector search using Ollama + LanceDB
- **Hybrid Search**: RRF fusion ranking combining keyword and semantic relevance
- **Filtering**: Filter by project, time range, session ID prefix

### MCP Integration
Search historical conversations directly in Claude Code:
- `search_history` - Search conversations (FTS/vector/hybrid)
- `get_session` - Get session details with pagination
- `get_recent_sessions` - Get recent sessions by project
- `list_projects` - List all projects

## Tech Stack

- **Backend**: Rust + Axum
- **Database**: SQLite + FTS5
- **Vector Store**: LanceDB
- **Embeddings**: Ollama (bge-m3)
- **Protocol**: HTTP + JSON-RPC (MCP)

## Quick Start

### Docker (Recommended)

```bash
docker run -d -p 3000:3000 \
  -v ~/.vimo/db:/data \
  -v ~/.claude/projects:/claude:ro \
  -v ~/.codex:/codex:ro \
  -v ~/.local/share/opencode:/opencode:ro \
  ghcr.io/vimo-ai/memex:0.0.1-beta.1
```

Mount the data sources you use:

| Mount | Data Source | Required? |
|-------|-------------|-----------|
| `~/.vimo/db:/data` | Database | ✅ Required |
| `~/.claude/projects:/claude` | Claude Code | As needed |
| `~/.codex:/codex` | Codex CLI | As needed |
| `~/.local/share/opencode:/opencode` | OpenCode | As needed |

Verify it's running:

```bash
curl http://localhost:3000/health          # → OK
curl http://localhost:3000/api/stats       # → {"projectCount":...}
```

### With Semantic Search

For semantic search, run Ollama on your host:

```bash
# Install Ollama and pull embedding model
ollama serve
ollama pull bge-m3

# Run Memex with Ollama access
docker run -d -p 3000:3000 \
  -v ~/.vimo/db:/data \
  -v ~/.claude/projects:/claude:ro \
  -e OLLAMA_API=http://host.docker.internal:11434 \
  ghcr.io/vimo-ai/memex:0.0.1-beta.1
```

**Linux note**: `host.docker.internal` works on Docker Desktop. On native Linux, use `--add-host=host.docker.internal:host-gateway` or your host's IP.

### Build from Source

```bash
git clone https://github.com/vimo-ai/memex.git
cd memex/memex-rs
cargo build --release
./target/release/memex serve
```

### Memex Lite (Zero-Dependency CLI)

For quick searches without running a server:

```bash
# Homebrew (macOS)
brew install vimo-ai/tap/memex

# Or download binary
curl -L https://github.com/vimo-ai/memex/releases/latest/download/memex-darwin-arm64.tar.gz | tar xz
sudo mv memex /usr/local/bin/
```

Usage:

```bash
# Search across all AI CLIs
memex search "authentication"

# Filter by CLI type
memex search "bug fix" --source claude

# List recent sessions
memex list -n 10

# View a specific session
memex view <session-id>

# Show available data sources
memex sources
```

Memex Lite directly reads JSONL files without any database, perfect for:
- Quick one-off searches
- New machines without full setup
- CI/CD environments
- Resource-constrained systems

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` | `10013` | HTTP server port |
| `VIMO_HOME` | `~/.vimo` | Base data directory (SQLite, LanceDB, backups) |
| `CLAUDE_PROJECTS_PATH` | `~/.claude/projects` | Claude Code session location |
| `CODEX_PATH` | `~/.codex` | Codex CLI session location |
| `OPENCODE_PATH` | `~/.local/share/opencode` | OpenCode session location |
| `GEMINI_PATH` | `~/.gemini/history` | Gemini CLI session location |
| `OLLAMA_API` | `http://localhost:11434` | Ollama API endpoint |
| `EMBEDDING_MODEL` | `bge-m3` | Ollama embedding model |
| `ENABLE_AI_CHAT` | `false` | Enable RAG Q&A feature |
| `CHAT_MODEL` | `qwen3:8b` | Ollama chat model for Q&A |

## Getting Started

1. **Start Memex** (see Quick Start above)

2. **Verify sessions imported** (auto-imports on startup)
   ```bash
   curl http://localhost:10013/api/stats
   # Should show sessionCount > 0
   ```

3. **Try a search**
   ```bash
   curl "http://localhost:10013/api/search?q=authentication&limit=5"
   ```

4. **Configure MCP** (optional) - see MCP section below

> **Note**: If `sessionCount` is 0, trigger manual collection: `curl -X POST http://localhost:10013/api/collect`

## MCP Configuration

Memex exposes MCP via HTTP at `http://localhost:10013/api/mcp`.

### For Claude Code (with mcp-remote)

Add to `~/.claude.json`:

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

### Verify MCP

```bash
curl -X POST http://localhost:10013/api/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

## API Reference

### Health & Stats
- `GET /health` - Health check
- `GET /api/stats` - Database statistics

### Projects & Sessions
- `GET /api/projects` - List all projects
- `GET /api/sessions` - List sessions
- `GET /api/sessions/{id}` - Get session details
- `GET /api/sessions/{id}/messages` - Get session messages

### Search
- `GET /api/search?q=...` - Full-text search
- `GET /api/search/semantic?q=...` - Semantic search
- `GET /api/search/hybrid?q=...` - Hybrid search

### Collection & Indexing
- `POST /api/collect` - Trigger session collection
- `POST /api/index` - Index specific session
- `GET /api/embedding/stats` - Indexing statistics

### MCP
- `POST /api/mcp` - MCP JSON-RPC endpoint
- `GET /api/mcp/info` - MCP server info

## Data Directory

```
~/.vimo/
├── db/
│   ├── ai-cli-session.db    # SQLite database
│   ├── lancedb/             # Vector storage
│   └── backups/             # Database backups
```

## FAQ

### How to trigger data import?

The service auto-imports on startup. Manual trigger:
```bash
curl -X POST http://localhost:10013/api/collect
```

### Semantic search not working?

1. Ensure Ollama is running: `ollama serve`
2. Pull the model: `ollama pull bge-m3`
3. Check status: `curl http://localhost:10013/api/embedding/status`

### No sessions showing up?

1. Check Claude Code path: `ls ~/.claude/projects/`
2. For Docker: ensure `CLAUDE_PROJECTS_PATH=/data/claude/projects` matches volume mount
3. Trigger collection: `curl -X POST http://localhost:10013/api/collect`

## Documentation

Full documentation: https://vimoai.dev/docs/memex

- [Installation](https://vimoai.dev/docs/memex/installation)
- [Configuration](https://vimoai.dev/docs/memex/configuration)
- [API Reference](https://vimoai.dev/docs/memex/api)
- [MCP Tools](https://vimoai.dev/docs/memex/mcp)
- [Architecture](https://vimoai.dev/docs/memex/architecture)

## License

MIT
