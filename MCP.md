# Memex MCP Server 使用指南

Memex MCP Server 为 Claude Code 提供历史对话查询能力。

## 功能特性

- 全文搜索历史对话
- 按项目过滤（通过 cwd 自动匹配）
- 获取会话详情和消息列表
- 会话内搜索并自动定位
- 查看最近会话
- 列出所有项目

## 安装和配置

### 1. 构建 MCP Server

```bash
cd /Users/higuaifan/Desktop/vimo/memex
pnpm install
pnpm run build:mcp
```

### 2. 配置 Claude Code

在 `~/.claude/settings.json` 中添加 MCP server 配置：

```json
{
  "mcpServers": {
    "memex": {
      "command": "node",
      "args": ["/Users/higuaifan/Desktop/vimo/memex/dist/mcp/index.js"]
    }
  }
}
```

### 3. 重启 Claude Code

配置生效后，Claude Code 会自动连接 Memex MCP Server。

## 可用工具

### search_history - 搜索历史对话

搜索历史对话内容，支持全文搜索。

**参数：**
- `query` (必需): 搜索关键词
- `mode` (可选): 搜索模式，可选 `fts`、`vector`、`hybrid`，默认 `fts`（目前仅支持 fts）
- `cwd` (可选): 当前工作目录，用于匹配项目。如果匹配不到则返回所有项目的结果
- `limit` (可选): 返回数量，默认 10

**返回：**
```json
{
  "results": [
    {
      "sessionId": "uuid",
      "messageIndex": 5,
      "content": "消息内容",
      "type": "user | assistant",
      "timestamp": "2024-11-26T00:00:00.000Z",
      "projectName": "项目名",
      "score": -1.5,
      "snippet": "...匹配[关键词]片段..."
    }
  ]
}
```

**示例：**
```
搜索所有提到 "NestJS" 的对话
搜索当前项目中关于 "MCP" 的讨论
```

### get_session - 获取会话详情

获取指定会话的消息列表，支持分页和会话内搜索。

**参数：**
- `sessionId` (必需): 会话 ID（完整 UUID 或前缀，如 "16b9dfba"）
- `offset` (可选): 消息偏移量，默认 0
- `limit` (可选): 返回消息数量，默认 10
- `search` (可选): 会话内搜索关键词，自动定位到第一个匹配的消息

**返回：**
```json
{
  "sessionId": "uuid",
  "projectName": "项目名",
  "totalMessages": 50,
  "currentOffset": 0,
  "messages": [
    {
      "index": 0,
      "type": "user",
      "content": "消息内容",
      "timestamp": "2024-11-26T00:00:00.000Z",
      "matched": false
    }
  ]
}
```

**示例：**
```
查看会话 16b9dfba 的前 10 条消息
在会话中搜索 "错误"，自动定位到匹配位置
```

### get_recent_sessions - 获取最近会话

获取最近更新的会话列表。

**参数：**
- `cwd` (可选): 当前工作目录，用于匹配项目。如果匹配不到则返回所有项目的会话
- `limit` (可选): 返回数量，默认 5

**返回：**
```json
{
  "sessions": [
    {
      "id": "uuid",
      "projectName": "项目名",
      "messageCount": 50,
      "updatedAt": "2024-11-26T00:00:00.000Z",
      "preview": "第一条用户消息的预览..."
    }
  ]
}
```

**示例：**
```
查看最近 5 个会话
查看当前项目的最近会话
```

### list_projects - 列出所有项目

列出所有已索引的项目及其会话数。

**参数：**
无

**返回：**
```json
{
  "projects": [
    {
      "id": 1,
      "name": "项目名",
      "path": "/path/to/project",
      "sessionCount": 10
    }
  ]
}
```

**示例：**
```
列出所有项目
查看哪些项目有会话记录
```

## 工作原理

### cwd 自动匹配

当你在某个项目目录下使用 Claude Code 时，传入 `cwd` 参数会自动匹配对应的项目：

1. Claude Code 会传递当前工作目录（如 `/Users/xxx/project`）
2. Memex 在 `projects` 表中查找 `path` 匹配的项目
3. 如果找到，只返回该项目的结果；找不到则返回所有项目的结果

### messageIndex 计算

`messageIndex` 表示消息在会话中的位置（0-based）：

```
会话消息列表:
[0] 第一条消息
[1] 第二条消息
[2] 第三条消息  <- messageIndex: 2
...
```

在搜索结果中返回 `messageIndex`，方便定位消息在会话中的位置。

### 会话内搜索

使用 `get_session` 的 `search` 参数时：

1. 先在会话内全文搜索，找到第一个匹配的消息
2. 自动调整 `offset`，让匹配的消息出现在返回结果的中间位置
3. 标记匹配的消息（`matched: true`）

## 数据要求

- 确保 Memex 主服务已运行，数据库文件存在于 `~/memex-data/memex.db`
- 至少已备份一次 Claude Code 会话数据
- 数据库包含 FTS5 全文索引

## 故障排查

### MCP Server 无法启动

检查：
1. 数据库文件是否存在：`ls ~/memex-data/memex.db`
2. Node.js 版本是否 >= 18
3. 构建产物是否存在：`ls dist/mcp/index.js`

### 搜索无结果

检查：
1. 数据库是否有数据：查看 Memex Web 界面或直接查询数据库
2. 搜索关键词是否正确
3. 是否因为 cwd 匹配导致范围过窄

### 向量搜索不可用

目前 MCP Server 仅支持 FTS 全文搜索。向量搜索需要：
1. Ollama 服务运行
2. 完成 embedding 索引构建
3. 后续版本将支持在 MCP 中使用向量搜索

## 开发和调试

### 本地测试

```bash
# 构建
pnpm run build:mcp

# 直接运行（会等待 stdin，用于调试）
node dist/mcp/index.js
```

### 查看日志

MCP Server 通过 stdio 通信，日志会输出到 stderr。Claude Code 会捕获这些日志。

### 修改后重新构建

```bash
pnpm run build:mcp
# 重启 Claude Code 使更改生效
```

## 示例对话

### 场景 1：搜索技术问题

```
User: 搜索我之前讨论过的 "NestJS 依赖注入" 问题
Claude: [调用 search_history，query="NestJS 依赖注入"]
        找到 3 条相关对话...
```

### 场景 2：回顾某个会话

```
User: 查看会话 16b9dfba 的内容
Claude: [调用 get_session，sessionId="16b9dfba"]
        这个会话是关于 MCP Server 实现的...
```

### 场景 3：查看最近工作

```
User: 我最近在这个项目做了什么？
Claude: [调用 get_recent_sessions，cwd=当前目录]
        最近的会话包括...
```

## 未来计划

- [ ] 支持向量语义搜索（需要 Ollama）
- [ ] 混合搜索模式（FTS + 向量）
- [ ] 更丰富的过滤条件（时间范围、消息类型等）
- [ ] 会话摘要和标签
- [ ] 基于历史的 RAG 问答

## 相关文档

- [Memex 设计文档](./DESIGN.md)
- [MCP 官方文档](https://modelcontextprotocol.io/)
- [Claude Code Hooks](https://docs.anthropic.com/en/docs/claude-code/hooks)
