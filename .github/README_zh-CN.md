<div align="center">
  <img src="assets/logo.svg" alt="Memex" width="200">
</div>

[English](../README.md) | [中文](README_zh-CN.md)

**统一记忆，全部 CLI，从不压缩，精确搜索。**

AI 编程助手的会话历史管理工具。再也不会丢失你的对话记录。

## 特性

- **按需搜索** - 由你决定何时搜索；自动注入是可选的
- **原始保留** - 原始消息始终保留；摘要是可选的附加层
- **多 CLI 支持** - Claude Code、Codex、OpenCode、Gemini 统一存储
- **强大搜索** - 全文搜索 (FTS5) + 语义向量 + 混合排序
- **MCP 集成** - 直接在 AI CLI 中搜索
- **REST API** - 集成到任何工作流
- **本地存储** - 所有数据保留在你的机器上

## 快速开始

### Full

```bash
brew install vimo-ai/tap/memex

memex search "任何你想搜索的内容"
memex list -n 10
```

### Lite

零依赖版本，直接读取本地会话数据：

```bash
brew install vimo-ai/tap/memex-lite
```

后台 agent (`vimo-agent`) 会在首次运行时自动下载。

### Full（Docker）

适用于 Linux 及其他平台：

```bash
docker run -d -p 10013:10013 \
  -v ~/.vimo:/data \
  -v ~/.claude/projects:/claude:ro \
  -v ~/.codex:/codex:ro \
  -v ~/.local/share/opencode:/opencode:ro \
  -v ~/.gemini/tmp:/gemini:ro \
  ghcr.io/vimo-ai/memex:latest
```

### 配置 MCP

```bash
# Claude Code
claude mcp add memex -- npx -y mcp-remote http://localhost:10013/api/mcp

# Codex
codex mcp add memex -- npx -y mcp-remote http://localhost:10013/api/mcp

# Gemini
gemini mcp add --transport http memex http://localhost:10013/api/mcp

# OpenCode - 编辑 ~/.config/opencode/opencode.json
# { "mcp": { "memex": { "type": "remote", "url": "http://localhost:10013/api/mcp" } } }
```

然后在 AI CLI 中搜索：

```
use memex search "able to search anything"
```

### Hooks（可选）

自动将相关记忆上下文注入 Claude Code 会话。详见 [Hook 文档](https://vimoai.dev/docs/memex/advanced/hooks)。

## 文档

https://vimoai.dev/docs/memex

## 社区

[![Discord](https://img.shields.io/badge/Discord-加入我们-5865F2?logo=discord&logoColor=white)](https://discord.gg/ZjznFAYSdE)

加入我们的 [Discord 服务器](https://discord.gg/ZjznFAYSdE)，参与讨论、获取支持和最新动态。
