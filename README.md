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

### Full

```bash
brew install vimo-ai/tap/memex

# Verify server is running
curl http://localhost:10013/health
```

### Lite

Zero-dependency CLI, reads local session data directly:

```bash
brew install vimo-ai/tap/memex-lite

memex search "anything you want"
memex list -n 10
```

### Docker

macOS / Linux:
```bash
docker run -d -p 10013:10013 \
  -v ~/.vimo:/data \
  -v ~/.claude/projects:/claude:ro \
  -v ~/.codex:/codex:ro \                              # 可选: Codex
  -v ~/.local/share/opencode:/opencode:ro \            # 可选: OpenCode
  -v ~/.gemini/tmp:/gemini:ro \                        # 可选: Gemini
  -e OLLAMA_HOST=http://host.docker.internal:11434 \   # 可选: 本机 Ollama (Docker Desktop)
  ghcr.io/vimo-ai/memex:latest
```

Windows (PowerShell):
```powershell
docker run -d -p 10013:10013 `
  -v "$env:USERPROFILE\.vimo:/data" `
  -v "$env:USERPROFILE\.claude\projects:/claude:ro" `
  -v "$env:USERPROFILE\.codex:/codex:ro" `             # 可选: Codex
  -v "$env:LOCALAPPDATA\opencode:/opencode:ro" `       # 可选: OpenCode
  -v "$env:USERPROFILE\.gemini\tmp:/gemini:ro" `       # 可选: Gemini
  -e OLLAMA_HOST=http://host.docker.internal:11434 `   # 可选: 本机 Ollama (Docker Desktop)
  ghcr.io/vimo-ai/memex:latest
```

Binary downloads available at [Releases](https://github.com/vimo-ai/memex/releases).

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

## Community

[![Discord](https://img.shields.io/badge/Discord-Join%20us-5865F2?logo=discord&logoColor=white)](https://discord.gg/ZjznFAYSdE)

Join our [Discord server](https://discord.gg/ZjznFAYSdE) for discussions, support, and updates.
