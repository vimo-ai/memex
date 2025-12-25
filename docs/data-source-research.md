# 数据源调研（Codex / Gemini CLI / Antigravity）

本文仅调研，不做开发。

## Codex CLI
- **位置**：`~/.codex/history.jsonl`（按行 `{session_id, ts, text}`，仅摘要）；`~/.codex/sessions/<年>/<月>/<日>/rollout-<ts>-<session_id>.jsonl`（完整事件流）。
- **会话元信息**：`rollout` 文件首行 `type=session_meta`，含 `cwd`、`git`、`cli_version` 等；项目归属需从 `cwd` 提取。
- **消息结构**：`response_item`（message/reasoning/function_call 等），`event_msg`（token_count 等），content 可能是数组；需要折叠成 user/assistant/tool 消息列表。
- **规模（当前本机）**：消息 825 条，会话 52 个，时间 2025-10-17~2025-11-27。
- **适配要点**：读取 `history.jsonl` 获取会话列表→逐个解析 `rollout-*.jsonl` 抽取 `cwd`、消息、工具调用，写入 Memex 模型，标记 `source=codex`。

## Gemini CLI（非 Antigravity）
- **位置**：`~/.gemini/tmp/<project_hash>/chats/session-*.json`（本机 27 个会话）。
- **结构**：`{sessionId, projectHash, startTime, lastUpdated, messages: [...]}`；消息字段 `id, timestamp, type(user|gemini), content, thoughts[], tokens{input/output/...}, toolCalls[]`。
- **项目归属**：`projectHash = getProjectHash(cwd)`；未直接存 cwd，需维护 hash↔cwd 映射或从 CLI 配置推断。
- **适配要点**：直接解析 JSON 写入 Memex，标记 `source=gemini-cli`；为查询/过滤增加 `projectHash` 索引或反查表。

## Antigravity（Gemini 编辑器版）
- **位置**：`~/.gemini/antigravity/conversations/*.pb`（高熵，非裸 proto，直解失败）；`~/.gemini/antigravity/brain/<id>/*.md` 与 `*.metadata.json`（明文工件，如 task.md / implementation_plan.md）。
- **现状**：开源 gemini-cli 代码只写 JSON（tmp/chats），未包含对话 `.pb` 的读写；`.pb` 很可能由 Antigravity Electron 应用自定义序列化/压缩/加密。
- **可行方向**：解包 Antigravity 应用（app.asar），搜索 conversations/.pb 的 encode/decode/proto/压缩逻辑；如拿到 schema/封装则可解析 `.pb`。短期可先索引 brain 目录的 Markdown/metadata，标记 `source=antigravity`，对话正文待破。
- **应用文件位置**：`/Applications/Antigravity.app/Contents/Resources/app`（unpacked），`node_modules.asar` 也在同目录；大量代码在 `out/*.js`。在 `out/main.js` 中可见路径常量：
  - app data 目录段：`[".gemini", yo.ideName]`
  - 规则：`rules/`
  - MCP 配置：`mcp_config.json`
  - user_settings：`user_settings.pb`
  - 全局记忆：`GEMINI.md`
  - 工件目录：`brain/`
  - “knowledge” 目录：`knowledge/`
  暂未在已浏览代码里找到 conversations `.pb` 的读写，仍需在 app.asar/out 里进一步定位。

## 统一适配思路
- Memex 域模型增加 `source`/`channel` 字段（claude / codex / gemini-cli / antigravity…），搜索/备份/RAG 可按来源过滤或聚合。
- Claude 适配器保持不变；新增 Codex、Gemini CLI 适配器；Antigravity 先收录 brain 工件，`.pb` 待解析后再补全。
