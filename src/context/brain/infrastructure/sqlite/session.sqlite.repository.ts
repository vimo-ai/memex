import { Inject, Injectable } from '@nestjs/common';
import { SQLITE_DB, SqliteDatabase } from './sqlite.provider';
import {
  ISessionRepository,
  SearchResult,
} from '../../domain/repositories/session.repository.interface';
import { SessionEntity, SessionStatus } from '../../domain/entities/session.entity';
import { MessageEntity, MessageType } from '../../domain/entities/message.entity';

/**
 * 会话表数据行类型
 */
interface SessionRow {
  id: string;
  project_id: number;
  status: string;
  source: string | null;
  channel: string | null;
  cwd: string | null;
  model: string | null;
  meta: string | null;
  message_count: number;
  file_mtime: number | null;
  file_size: number | null;
  created_at: string;
  updated_at: string;
}

/**
 * 消息表数据行类型
 */
interface MessageRow {
  id: number;
  uuid: string;
  session_id: string;
  type: string;
  source: string | null;
  channel: string | null;
  model: string | null;
  tool_call_id: string | null;
  tool_name: string | null;
  tool_args: string | null;
  raw: string | null;
  meta: string | null;
  content: string;
  timestamp: string | null;
  created_at: string;
}

/**
 * FTS 搜索结果行类型
 */
interface FtsSearchRow extends MessageRow {
  snippet: string;
  rank: number;
}

/**
 * 会话仓储 SQLite 实现
 */
@Injectable()
export class SessionSqliteRepository implements ISessionRepository {
  constructor(@Inject(SQLITE_DB) private readonly db: SqliteDatabase) {}

  // ========== 会话操作 ==========

  /**
   * 保存会话（UPSERT）
   */
  saveSession(session: SessionEntity): SessionEntity {
    const stmt = this.db.prepare(`
      INSERT INTO sessions (id, project_id, status, source, channel, cwd, model, meta, message_count, file_mtime, file_size, created_at, updated_at)
      VALUES (@id, @projectId, @status, @source, @channel, @cwd, @model, @meta, @messageCount, @fileMtime, @fileSize, @createdAt, @updatedAt)
      ON CONFLICT(id) DO UPDATE SET
        status = excluded.status,
        source = excluded.source,
        channel = excluded.channel,
        cwd = excluded.cwd,
        model = excluded.model,
        meta = excluded.meta,
        message_count = excluded.message_count,
        file_mtime = excluded.file_mtime,
        file_size = excluded.file_size,
        updated_at = @updatedAt
      RETURNING *
    `);

    const row = stmt.get({
      id: session.id,
      projectId: session.projectId,
      status: session.status,
      source: session.source,
      channel: session.channel ?? null,
      cwd: session.cwd ?? null,
      model: session.model ?? null,
      meta: session.meta ? JSON.stringify(session.meta) : null,
      messageCount: session.messageCount,
      fileMtime: session.fileMtime ?? null,
      fileSize: session.fileSize ?? null,
      createdAt: session.createdAt?.toISOString() ?? new Date().toISOString(),
      updatedAt: session.updatedAt?.toISOString() ?? new Date().toISOString(),
    }) as SessionRow;

    return this.sessionRowToEntity(row);
  }

  /**
   * 根据 ID 查找会话
   */
  findSessionById(id: string): SessionEntity | null {
    const stmt = this.db.prepare('SELECT * FROM sessions WHERE id = ?');
    const row = stmt.get(id) as SessionRow | undefined;
    return row ? this.sessionRowToEntity(row) : null;
  }

  /**
   * 根据项目 ID 查找所有会话
   */
  findSessionsByProjectId(projectId: number): SessionEntity[] {
    const stmt = this.db.prepare(
      'SELECT * FROM sessions WHERE project_id = ? ORDER BY updated_at DESC',
    );
    const rows = stmt.all(projectId) as SessionRow[];
    return rows.map((row) => this.sessionRowToEntity(row));
  }

  /**
   * 获取所有会话
   */
  findAllSessions(): SessionEntity[] {
    const stmt = this.db.prepare('SELECT * FROM sessions ORDER BY updated_at DESC');
    const rows = stmt.all() as SessionRow[];
    return rows.map((row) => this.sessionRowToEntity(row));
  }

  /**
   * 删除会话（关联的消息会通过外键级联删除）
   */
  deleteSession(id: string): boolean {
    const stmt = this.db.prepare('DELETE FROM sessions WHERE id = ?');
    const result = stmt.run(id);
    return result.changes > 0;
  }

  /**
   * 统计会话数量
   */
  countSessions(): number {
    const stmt = this.db.prepare('SELECT COUNT(*) as count FROM sessions');
    const result = stmt.get() as { count: number };
    return result.count;
  }

  /**
   * 重置指定条件的会话的 file_mtime（用于强制重新采集）
   */
  resetFileMtime(condition?: string): number {
    let sql = 'UPDATE sessions SET file_mtime = NULL';
    if (condition) {
      sql += ` WHERE ${condition}`;
    }
    const stmt = this.db.prepare(sql);
    const result = stmt.run();
    return result.changes;
  }

  /**
   * 批量更新会话的 project_id（用于合并项目）
   */
  updateProjectId(fromProjectId: number, toProjectId: number): number {
    const stmt = this.db.prepare(
      'UPDATE sessions SET project_id = ? WHERE project_id = ?',
    );
    const result = stmt.run(toProjectId, fromProjectId);
    return result.changes;
  }

  // ========== 消息操作 ==========

  /**
   * 批量保存消息（UPSERT）
   * @returns 实际插入的数量
   */
  saveMessages(messages: MessageEntity[]): number {
    if (messages.length === 0) return 0;

    const stmt = this.db.prepare(`
      INSERT INTO messages (uuid, session_id, type, source, channel, model, tool_call_id, tool_name, tool_args, raw, meta, content, timestamp)
      VALUES (@uuid, @sessionId, @type, @source, @channel, @model, @toolCallId, @toolName, @toolArgs, @raw, @meta, @content, @timestamp)
      ON CONFLICT(session_id, uuid) DO NOTHING
    `);

    const insertMany = this.db.transaction((msgs: MessageEntity[]) => {
      let inserted = 0;
      for (const msg of msgs) {
        const result = stmt.run({
          uuid: msg.uuid,
          sessionId: msg.sessionId,
          type: msg.type,
          source: msg.source ?? null,
          channel: msg.channel ?? null,
          model: msg.model ?? null,
          toolCallId: msg.toolCallId ?? null,
          toolName: msg.toolName ?? null,
          toolArgs: msg.toolArgs ?? null,
          raw: msg.raw ?? null,
          meta: msg.meta ? JSON.stringify(msg.meta) : null,
          content: msg.content,
          timestamp: msg.timestamp?.toISOString() ?? null,
        });
        if (result.changes > 0) inserted++;
      }
      return inserted;
    });

    return insertMany(messages);
  }

  /**
   * 根据会话 ID 查找所有消息
   */
  findMessagesBySessionId(sessionId: string): MessageEntity[] {
    const stmt = this.db.prepare(
      'SELECT * FROM messages WHERE session_id = ? ORDER BY id ASC',
    );
    const rows = stmt.all(sessionId) as MessageRow[];
    return rows.map((row) => this.messageRowToEntity(row));
  }

  /**
   * 统计消息数量
   */
  countMessages(): number {
    const stmt = this.db.prepare('SELECT COUNT(*) as count FROM messages');
    const result = stmt.get() as { count: number };
    return result.count;
  }

  /**
   * 获取会话的最后一条消息 UUID
   */
  getLastMessageUuid(sessionId: string): string | null {
    const stmt = this.db.prepare(
      'SELECT uuid FROM messages WHERE session_id = ? ORDER BY id DESC LIMIT 1',
    );
    const result = stmt.get(sessionId) as { uuid: string } | undefined;
    return result?.uuid ?? null;
  }

  // ========== 搜索操作 ==========

  /**
   * 全文搜索消息
   *
   * @param query 搜索关键词
   * @param limit 返回数量限制
   * @param startDate 开始时间（ISO 格式）
   * @param endDate 结束时间（ISO 格式）
   * @param projectId 项目 ID（可选，用于过滤特定项目）
   */
  searchMessages(
    query: string,
    limit: number = 50,
    startDate?: string,
    endDate?: string,
    projectId?: number,
  ): SearchResult[] {
    // 清理 FTS5 特殊字符，防止语法错误
    const sanitizedQuery = this.sanitizeFts5Query(query);

    // 如果清理后为空，返回空结果
    if (!sanitizedQuery) {
      return [];
    }

    try {
      // 构建 WHERE 条件
      const conditions = ['messages_fts MATCH ?'];
      const params: (string | number)[] = [sanitizedQuery];

      // 添加时间范围过滤
      if (startDate) {
        conditions.push('m.timestamp >= ?');
        params.push(startDate);
      }
      if (endDate) {
        conditions.push('m.timestamp <= ?');
        params.push(endDate);
      }

      // 添加项目过滤
      if (projectId !== undefined) {
        conditions.push('s.project_id = ?');
        params.push(projectId);
      }

      const whereClause = conditions.join(' AND ');

      const stmt = this.db.prepare(`
        SELECT
          m.*,
          snippet(messages_fts, 0, '[', ']', '...', 64) as snippet,
          rank
        FROM messages_fts
        JOIN messages m ON messages_fts.rowid = m.id
        JOIN sessions s ON m.session_id = s.id
        WHERE ${whereClause}
        ORDER BY rank
        LIMIT ?
      `);

      params.push(limit);
      const rows = stmt.all(...params) as FtsSearchRow[];

      return rows.map((row) => ({
        message: this.messageRowToEntity(row),
        snippet: row.snippet,
        rank: row.rank,
      }));
    } catch (error) {
      // FTS 查询失败时返回空数组，让向量搜索兜底
      console.error(`[FTS] 查询失败: query="${query}", sanitized="${sanitizedQuery}"`, error);
      return [];
    }
  }

  /**
   * 根据 ID 前缀搜索会话
   */
  searchSessionsByIdPrefix(idPrefix: string, limit: number = 20): SessionEntity[] {
    const stmt = this.db.prepare(`
      SELECT * FROM sessions
      WHERE id LIKE ?
      ORDER BY updated_at DESC
      LIMIT ?
    `);
    const rows = stmt.all(`${idPrefix}%`, limit) as SessionRow[];
    return rows.map((row) => this.sessionRowToEntity(row));
  }

  // ========== 私有方法 ==========

  /**
   * 清理 FTS5 查询字符串，移除特殊字符并用 OR 连接
   *
   * FTS5 特殊字符包括：" - ( ) * : < > = . [ ] { } ^ ~ ! @ # $ % & ? / \ 等
   * 策略：
   * 1. 移除所有特殊字符
   * 2. 分词后用 OR 连接（宽松匹配，让调用方自行筛选）
   *
   * @param query 原始查询字符串
   * @returns 清理后的 FTS5 查询字符串
   */
  private sanitizeFts5Query(query: string): string {
    // 移除 FTS5 特殊字符和标点符号
    const sanitized = query
      .replace(/["()*:<>=\-.\[\]{}^~!@#$%&?/\\,;'`|+]/g, ' ')
      .replace(/\s+/g, ' ')
      .trim();

    // 如果清理后为空，返回空（调用方会处理）
    if (!sanitized) {
      return '';
    }

    // 分词并用 OR 连接
    const terms = sanitized.split(' ').filter((t) => t.length > 0);
    if (terms.length === 0) {
      return '';
    }

    // 单个词直接返回，多个词用 OR 连接
    if (terms.length === 1) {
      return terms[0];
    }

    return terms.join(' OR ');
  }

  /**
   * 会话数据行转换为实体
   */
  private sessionRowToEntity(row: SessionRow): SessionEntity {
    return new SessionEntity({
      id: row.id,
      projectId: row.project_id,
      status: row.status as SessionStatus,
      source: row.source ?? 'claude',
      channel: row.channel ?? undefined,
      cwd: row.cwd ?? undefined,
      model: row.model ?? undefined,
      meta: row.meta ? JSON.parse(row.meta) : undefined,
      messageCount: row.message_count,
      fileMtime: row.file_mtime ?? undefined,
      fileSize: row.file_size ?? undefined,
      createdAt: new Date(row.created_at),
      updatedAt: new Date(row.updated_at),
    });
  }

  /**
   * 消息数据行转换为实体
   */
  private messageRowToEntity(row: MessageRow): MessageEntity {
    return new MessageEntity({
      id: row.id,
      uuid: row.uuid,
      sessionId: row.session_id,
      type: row.type as MessageType,
      source: row.source ?? 'claude',
      channel: row.channel ?? undefined,
      model: row.model ?? undefined,
      toolCallId: row.tool_call_id ?? undefined,
      toolName: row.tool_name ?? undefined,
      toolArgs: row.tool_args ?? undefined,
      raw: row.raw ?? undefined,
      meta: row.meta ? JSON.parse(row.meta) : undefined,
      content: row.content,
      timestamp: row.timestamp ? new Date(row.timestamp) : undefined,
      createdAt: new Date(row.created_at),
    });
  }
}
