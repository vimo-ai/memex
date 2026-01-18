# Memex

**One Memory. All CLIs. Never Compacted. Exact Search.**

Session history management for AI coding assistants. Never lose your conversations again.

## Supported Tools

- Claude Code
- Codex CLI
- OpenCode
- Gemini CLI

## Quick Start

### Lite (Quick Search)

Reads local session data directly, no server needed:

```bash
brew install vimo-ai/tap/memex

memex search "authentication"
memex list -n 10
```

### Full (Recommended)

**1. Start Docker**

```bash
docker run -d -p 10013:10013 \
  -v ~/.vimo:/data \
  -v ~/.claude/projects:/claude:ro \
  -v ~/.codex:/codex:ro \
  -v ~/.local/share/opencode:/opencode:ro \
  -v ~/.gemini/tmp:/gemini:ro \
  ghcr.io/vimo-ai/memex:latest
```

**2. Configure MCP**

```bash
# Claude Code
claude mcp add memex -- npx -y mcp-remote http://localhost:10013/api/mcp

# Codex
codex mcp add memex -- npx -y mcp-remote http://localhost:10013/api/mcp

# Gemini
gemini mcp add --transport http memex http://localhost:10013/api/mcp
```

**3. Search in Your AI CLI**

```
use memex search "anything you want"
```

## Documentation

https://vimoai.dev/docs/memex

## License

MIT
