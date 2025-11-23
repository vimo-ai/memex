import { SessionEntity } from '../entities/session.entity';
import { MessageEntity } from '../entities/message.entity';

/**
 * 会话仓储接口注入 Token
 */
export const SESSION_REPOSITORY = Symbol('SESSION_REPOSITORY');

/**
 * 全文搜索结果
 */
export interface SearchResult {
  /** 消息实体 */
  message: MessageEntity;
  /** 匹配的高亮片段 */
  snippet?: string;
  /** 相关性得分 */
  rank?: number;
}

/**
 * 会话仓储接口
 *
 * 定义会话和消息数据访问的契约
 */
export interface ISessionRepository {
  // ========== 会话操作 ==========

  /**
   * 保存会话
   * - 如果会话不存在则创建
   * - 如果已存在则更新
   */
  saveSession(session: SessionEntity): SessionEntity;

  /**
   * 根据 ID 查找会话
   */
  findSessionById(id: string): SessionEntity | null;

  /**
   * 根据项目 ID 查找所有会话
   */
  findSessionsByProjectId(projectId: number): SessionEntity[];

  /**
   * 获取所有会话
   */
  findAllSessions(): SessionEntity[];

  /**
   * 删除会话（同时删除关联的消息）
   */
  deleteSession(id: string): boolean;

  /**
   * 统计会话数量
   */
  countSessions(): number;

  // ========== 消息操作 ==========

  /**
   * 批量保存消息
   * - 使用 UPSERT 避免重复
   */
  saveMessages(messages: MessageEntity[]): number;

  /**
   * 根据会话 ID 查找所有消息
   */
  findMessagesBySessionId(sessionId: string): MessageEntity[];

  /**
   * 统计消息数量
   */
  countMessages(): number;

  /**
   * 获取会话的最后一条消息 UUID
   * 用于增量导入时判断从哪里开始
   */
  getLastMessageUuid(sessionId: string): string | null;

  // ========== 搜索操作 ==========

  /**
   * 全文搜索消息
   * @param query 搜索关键词
   * @param limit 返回数量限制
   */
  searchMessages(query: string, limit?: number): SearchResult[];
}
