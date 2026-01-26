<div align="center">
  <img src=".github/assets/logo.svg" alt="Memex" width="200">
</div>

[English](README.md) | [中文](.github/README_zh-CN.md)

**One Memory. All CLIs. Never Compacted. Exact Search.**

Session history management for AI coding assistants. Never lose your conversations again.

## Features

- **On-demand search** - You control when to search; automatic injection is opt-in
- **Original preservation** - Raw messages always kept; summaries are optional layers
- **Multi-CLI support** - Claude Code, Codex, OpenCode, Gemini in one database
- **Powerful search** - Full-text (FTS5) + semantic vectors + hybrid ranking
- **MCP integration** - Search directly from your AI CLI
- **REST API** - Integrate into any workflow
- **Local storage** - All data stays on your machine

## Quick Start

### Lite (Quick Search)

Reads local session data directly, no server needed:

```bash
brew install vimo-ai/tap/memex

memex search "authentication"
memex list -n 10
```

### Full (macOS)

```bash
mkdir -p ~/.vimo/bin && curl -L -o ~/.vimo/bin/memex \
  https://github.com/vimo-ai/memex/releases/latest/download/memex-darwin-arm64 && \
  chmod +x ~/.vimo/bin/memex && ~/.vimo/bin/memex
```

The background agent (`vimo-agent`) will be downloaded automatically on first run.

### Full (Docker)

For Linux and other platforms:

```bash
docker run -d -p 10013:10013 \
  -v ~/.vimo:/data \
  -v ~/.claude/projects:/claude:ro \
  -v ~/.codex:/codex:ro \
  -v ~/.local/share/opencode:/opencode:ro \
  -v ~/.gemini/tmp:/gemini:ro \
  ghcr.io/vimo-ai/memex:latest
```

### Configure MCP

```bash
# Claude Code
claude mcp add memex -- npx -y mcp-remote http://localhost:10013/api/mcp

# Codex
codex mcp add memex -- npx -y mcp-remote http://localhost:10013/api/mcp

# Gemini
gemini mcp add --transport http memex http://localhost:10013/api/mcp

# OpenCode - edit ~/.config/opencode/opencode.json
# { "mcp": { "memex": { "type": "remote", "url": "http://localhost:10013/api/mcp" } } }
```

Then search in your AI CLI:

```
use memex search "anything you want"
```

### Hooks (Optional)

Auto-inject relevant memory context into Claude Code sessions. See [Hook Documentation](https://vimoai.dev/docs/memex/advanced/hooks) for setup.

## Documentation

https://vimoai.dev/docs/memex
