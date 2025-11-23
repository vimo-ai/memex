# CLAUDE.md

## 项目简介

Memex 是一个 Claude Code 会话历史管理系统，解决以下问题：
1. Claude Code 本地数据 30 天过期
2. 历史对话难以检索
3. 知识没有沉淀

## 共享代码

Memex 与 Vlaude 共享 Claude Code 解析的核心代码：

- **共享包位置**: `/Users/higuaifan/Desktop/hi/小工具/claude/packages/shared-core`
- **Vlaude 项目**: `/Users/higuaifan/Desktop/hi/小工具/claude`

可复用的模块：
- `ClaudeSessionAR` - Session 聚合根
- `ClaudeMessageEntity` - Message 实体
- JSONL 解析逻辑
- 路径编码/解码
- 文件监听

## 技术栈

- **框架**: NestJS (DDD 架构)
- **存储**: SQLite + FTS5
- **通信**: HTTP
- **后续**: LanceDB (向量) + Ollama (embedding/LLM)

## 架构风格

参考 janghood-next 的 DDD 分层：
- `context/` - Bounded Context
- `api/controllers/` - 控制器
- `application/services/` - 应用服务
- `domain/entities/` - 领域实体
- `infrastructure/` - 基础设施

## 开发优先级

**当前阶段: Phase 0 + 1**

1. 扫描 `~/.claude/projects/` 的 JSONL 文件
2. 解析并存储到 SQLite
3. 每日增量备份
4. FTS5 全文搜索
5. HTTP API

## 数据源

Claude Code 会话存储在:
```
~/.claude/projects/{encoded-project-path}/{session-uuid}.jsonl
```

JSONL 每行是一个 JSON 对象，关键类型:
- `type: "user"` - 用户消息
- `type: "message"` (role: assistant) - Claude 回复

## 重要约定

- 使用中文注释和文档
- 遵循 NestJS + DDD 最佳实践
- 先实现核心功能，不要过度设计
- Phase 2+ 的功能（向量、RAG）暂不实现

## 设计文档

详见 [DESIGN.md](./DESIGN.md)
