/**
 * SQLite 表结构定义
 *
 * 包含：
 * - projects: 项目表
 * - sessions: 会话表
 * - messages: 消息表
 * - messages_fts: FTS5 全文索引表
 */

/** 项目表 */
export const CREATE_PROJECTS_TABLE = `
CREATE TABLE IF NOT EXISTS projects (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  path TEXT NOT NULL,                  -- 真实项目路径
  name TEXT NOT NULL,                  -- 项目名称
  encoded_dir_name TEXT,               -- 编码的目录名（Claude Code 使用的目录名，可为空）
  source TEXT DEFAULT 'claude',        -- 数据来源
  created_at TEXT DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT DEFAULT CURRENT_TIMESTAMP,
  UNIQUE(path, source)                 -- path + source 组合唯一
)`;

/** 项目表索引 */
export const CREATE_PROJECTS_INDEX = `
CREATE INDEX IF NOT EXISTS idx_projects_encoded_dir ON projects(encoded_dir_name)`;

/** 会话表 */
export const CREATE_SESSIONS_TABLE = `
CREATE TABLE IF NOT EXISTS sessions (
  id TEXT PRIMARY KEY,                 -- session uuid
  project_id INTEGER NOT NULL,
  status TEXT DEFAULT 'active',        -- active/closed/archived
  source TEXT DEFAULT 'claude',        -- 数据来源
  channel TEXT,                        -- 渠道/子来源
  cwd TEXT,                            -- 工作目录
  model TEXT,                          -- 默认模型
  meta TEXT,                           -- 额外元信息（JSON 字符串）
  message_count INTEGER DEFAULT 0,
  file_mtime INTEGER,                  -- 文件修改时间戳（用于增量检测）
  file_size INTEGER,                   -- 文件大小（用于增量检测）
  created_at TEXT DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT DEFAULT CURRENT_TIMESTAMP,
  FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
)`;

/** 会话表索引 */
export const CREATE_SESSIONS_INDEX = `
CREATE INDEX IF NOT EXISTS idx_sessions_project_id ON sessions(project_id)`;

/** 消息表 */
export const CREATE_MESSAGES_TABLE = `
CREATE TABLE IF NOT EXISTS messages (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  uuid TEXT NOT NULL,
  session_id TEXT NOT NULL,
  type TEXT NOT NULL,                  -- user/assistant/tool
  source TEXT DEFAULT 'claude',        -- 数据来源
  channel TEXT,                        -- 渠道/子来源
  model TEXT,                          -- 模型名称
  tool_call_id TEXT,                   -- 工具调用 ID
  tool_name TEXT,                      -- 工具名称
  tool_args TEXT,                      -- 工具参数
  raw TEXT,                            -- 原始内容
  meta TEXT,                           -- 额外元信息（JSON）
  content TEXT NOT NULL,               -- 消息内容
  timestamp TEXT,                      -- 消息时间戳
  created_at TEXT DEFAULT CURRENT_TIMESTAMP,
  FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
  UNIQUE(session_id, uuid)
)`;

/** 消息表索引 */
export const CREATE_MESSAGES_INDEX = `
CREATE INDEX IF NOT EXISTS idx_messages_session_id ON messages(session_id)`;

/** FTS5 全文索引表 */
export const CREATE_MESSAGES_FTS = `
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
  content,
  content='messages',
  content_rowid='id'
)`;

/** FTS 插入触发器 */
export const CREATE_FTS_INSERT_TRIGGER = `
CREATE TRIGGER IF NOT EXISTS messages_fts_insert AFTER INSERT ON messages BEGIN
  INSERT INTO messages_fts(rowid, content) VALUES (new.id, new.content);
END`;

/** FTS 删除触发器 */
export const CREATE_FTS_DELETE_TRIGGER = `
CREATE TRIGGER IF NOT EXISTS messages_fts_delete AFTER DELETE ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, content) VALUES('delete', old.id, old.content);
END`;

/** FTS 更新触发器 */
export const CREATE_FTS_UPDATE_TRIGGER = `
CREATE TRIGGER IF NOT EXISTS messages_fts_update AFTER UPDATE ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, content) VALUES('delete', old.id, old.content);
  INSERT INTO messages_fts(rowid, content) VALUES (new.id, new.content);
END`;

/** 所有表创建语句（按顺序执行） */
export const ALL_SCHEMA_STATEMENTS = [
  CREATE_PROJECTS_TABLE,
  CREATE_PROJECTS_INDEX,
  CREATE_SESSIONS_TABLE,
  CREATE_SESSIONS_INDEX,
  CREATE_MESSAGES_TABLE,
  CREATE_MESSAGES_INDEX,
  CREATE_MESSAGES_FTS,
  CREATE_FTS_INSERT_TRIGGER,
  CREATE_FTS_DELETE_TRIGGER,
  CREATE_FTS_UPDATE_TRIGGER,
];
