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
      INSERT INTO sessions (id, project_id, status, message_count, file_mtime, file_size, created_at, updated_at)
      VALUES (@id, @projectId, @status, @messageCount, @fileMtime, @fileSize, @createdAt, @updatedAt)
      ON CONFLICT(id) DO UPDATE SET
        status = excluded.status,
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

  // ========== 消息操作 ==========

  /**
   * 批量保存消息（UPSERT）
   * @returns 实际插入的数量
   */
  saveMessages(messages: MessageEntity[]): number {
    if (messages.length === 0) return 0;

    const stmt = this.db.prepare(`
      INSERT INTO messages (uuid, session_id, type, content, timestamp)
      VALUES (@uuid, @sessionId, @type, @content, @timestamp)
      ON CONFLICT(session_id, uuid) DO NOTHING
    `);

    const insertMany = this.db.transaction((msgs: MessageEntity[]) => {
      let inserted = 0;
      for (const msg of msgs) {
        const result = stmt.run({
          uuid: msg.uuid,
          sessionId: msg.sessionId,
          type: msg.type,
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
   */
  searchMessages(query: string, limit: number = 50): SearchResult[] {
    // 使用 FTS5 snippet 函数高亮匹配内容
    // -1 表示 content 列，'[' ']' 是高亮标记，'...' 是省略号，64 是 snippet 最大长度
    const stmt = this.db.prepare(`
      SELECT
        m.*,
        snippet(messages_fts, 0, '[', ']', '...', 64) as snippet,
        rank
      FROM messages_fts
      JOIN messages m ON messages_fts.rowid = m.id
      WHERE messages_fts MATCH ?
      ORDER BY rank
      LIMIT ?
    `);

    const rows = stmt.all(query, limit) as FtsSearchRow[];

    return rows.map((row) => ({
      message: this.messageRowToEntity(row),
      snippet: row.snippet,
      rank: row.rank,
    }));
  }

  // ========== 私有方法 ==========

  /**
   * 会话数据行转换为实体
   */
  private sessionRowToEntity(row: SessionRow): SessionEntity {
    return new SessionEntity({
      id: row.id,
      projectId: row.project_id,
      status: row.status as SessionStatus,
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
      content: row.content,
      timestamp: row.timestamp ? new Date(row.timestamp) : undefined,
      createdAt: new Date(row.created_at),
    });
  }
}
