import { Injectable, Inject, Logger, OnModuleInit } from '@nestjs/common';
import type { Connection, Table } from '@lancedb/lancedb';
import { LANCEDB_CONNECTION } from './lancedb.provider';
import { EMBEDDING_DIMENSION } from '../../application/services/embedding.service';

/** 向量表名 */
const TABLE_NAME = 'message_vectors';

/**
 * 向量记录结构
 */
export interface VectorRecord {
  /** 消息 ID（SQLite 中的 id） */
  messageId: number;
  /** 分片索引（从 0 开始），单条消息可能有多个分片 */
  chunkIndex: number;
  /** 消息 UUID */
  uuid: string;
  /** 会话 ID */
  sessionId: string;
  /** 项目 ID */
  projectId: string;
  /** 分片内容（用于 embedding 和检索） */
  content: string;
  /** 完整消息内容（可选，用于展示） */
  fullContent?: string;
  /** 消息类型 */
  messageType: string;
  /** 分片类型 */
  chunkType: 'code' | 'text';
  /** 消息时间戳（ISO 格式字符串） */
  timestamp?: string;
  /** 向量 */
  vector: number[];
}

/**
 * 向量搜索结果
 */
export interface VectorSearchResult {
  messageId: number;
  chunkIndex: number;
  uuid: string;
  sessionId: string;
  projectId: string;
  /** 分片内容 */
  content: string;
  /** 完整消息内容（如果存储了） */
  fullContent?: string;
  messageType: string;
  chunkType: 'code' | 'text';
  /** 消息时间戳 */
  timestamp: string;
  /** 距离（越小越相似） */
  distance: number;
  /** 相似度（0-1，越大越相似） */
  similarity: number;
}

/**
 * 向量仓储接口 Token
 */
export const VECTOR_REPOSITORY = Symbol('VECTOR_REPOSITORY');

/**
 * LanceDB 向量仓储
 *
 * 职责：
 * - 存储消息向量
 * - 向量相似度搜索
 * - 增量更新
 */
@Injectable()
export class VectorLanceDbRepository implements OnModuleInit {
  private readonly logger = new Logger(VectorLanceDbRepository.name);
  private table: Table | null = null;

  constructor(
    @Inject(LANCEDB_CONNECTION)
    private readonly db: Connection,
  ) {}

  async onModuleInit() {
    await this.ensureTable();
  }

  /**
   * 确保向量表存在
   */
  private async ensureTable(): Promise<void> {
    const tableNames = await this.db.tableNames();

    if (tableNames.includes(TABLE_NAME)) {
      this.table = await this.db.openTable(TABLE_NAME);
      this.logger.log(`打开向量表: ${TABLE_NAME}`);
    } else {
      // 创建空表需要至少一条数据，先不创建，等第一次插入时创建
      this.logger.log(`向量表 ${TABLE_NAME} 不存在，将在首次插入时创建`);
    }
  }

  /**
   * 批量插入向量记录
   */
  async upsertBatch(records: VectorRecord[]): Promise<void> {
    if (records.length === 0) return;

    // 转换为 LanceDB 格式（暂不使用 timestamp，保持向后兼容）
    const data = records.map((r) => ({
      message_id: r.messageId,
      chunk_index: r.chunkIndex,
      uuid: r.uuid,
      session_id: r.sessionId,
      project_id: r.projectId,
      content: r.content,
      full_content: r.fullContent || null,
      message_type: r.messageType,
      chunk_type: r.chunkType,
      // timestamp: r.timestamp || null,  // MVP 阶段暂不使用，避免 schema 不兼容
      vector: r.vector,
    }));

    if (!this.table) {
      // 首次创建表
      this.table = await this.db.createTable(TABLE_NAME, data);
      this.logger.log(`创建向量表，初始记录数: ${records.length}`);
    } else {
      // 追加数据（成功时不打印，失败时会抛异常）
      await this.table.add(data);
    }
  }

  /**
   * 向量相似度搜索
   *
   * @param vector 查询向量
   * @param limit 返回数量
   * @param filter 过滤条件（projectId、sessionId、时间范围等）
   * @param minSimilarity 最低相似度阈值，默认 0
   */
  async search(
    vector: number[],
    limit: number = 10,
    filter?: {
      projectId?: string;
      sessionId?: string;
      startDate?: string;
      endDate?: string;
    },
    minSimilarity: number = 0, // 暂时禁用阈值过滤
  ): Promise<VectorSearchResult[]> {
    if (!this.table) {
      this.logger.warn('向量表不存在，返回空结果');
      return [];
    }

    // 多取一些，过滤后可能不够
    let query = this.table.search(vector).limit(limit * 3);

    // 应用过滤条件
    if (filter?.projectId) {
      query = query.where(`project_id = '${filter.projectId}'`);
    }
    if (filter?.sessionId) {
      query = query.where(`session_id = '${filter.sessionId}'`);
    }
    if (filter?.startDate) {
      query = query.where(`timestamp >= '${filter.startDate}'`);
    }
    if (filter?.endDate) {
      query = query.where(`timestamp <= '${filter.endDate}'`);
    }

    const results = await query.toArray();

    // 调试日志：查看原始搜索结果
    if (results.length > 0) {
      const distances = results.slice(0, 5).map((r) => r._distance);
      this.logger.log(`原始搜索结果: ${results.length} 条, 前5个距离: [${distances.join(', ')}]`);
    } else {
      this.logger.warn('LanceDB 搜索返回 0 条原始结果');
    }

    return results
      .map((row) => ({
        messageId: row.message_id as number,
        chunkIndex: row.chunk_index as number,
        uuid: row.uuid as string,
        sessionId: row.session_id as string,
        projectId: row.project_id as string,
        content: row.content as string,
        fullContent: row.full_content as string | undefined,
        messageType: row.message_type as string,
        chunkType: row.chunk_type as 'code' | 'text',
        timestamp: row.timestamp as string,
        distance: row._distance as number,
        // 将距离转换为相似度（LanceDB 默认使用 L2 距离）
        // 使用简化的转换：similarity = 1 / (1 + distance)
        similarity: 1 / (1 + (row._distance as number)),
      }))
      .filter((r) => r.similarity >= minSimilarity) // 过滤低相似度结果
      .slice(0, limit); // 截取到请求的数量
  }

  /**
   * 获取已索引的消息 ID 列表（去重）
   */
  async getIndexedMessageIds(): Promise<Set<number>> {
    if (!this.table) {
      return new Set();
    }

    const results = await this.table.query().select(['message_id']).toArray();
    return new Set(results.map((r) => r.message_id as number));
  }

  /**
   * 获取索引统计信息
   */
  async getStats(): Promise<{ totalRecords: number; tableExists: boolean }> {
    if (!this.table) {
      return { totalRecords: 0, tableExists: false };
    }

    const count = await this.table.countRows();
    return { totalRecords: count, tableExists: true };
  }

  /**
   * 删除指定消息的向量
   */
  async deleteByMessageIds(messageIds: number[]): Promise<void> {
    if (!this.table || messageIds.length === 0) return;

    const idList = messageIds.join(',');
    await this.table.delete(`message_id IN (${idList})`);
    this.logger.debug(`删除 ${messageIds.length} 条向量记录`);
  }

  /**
   * 清空所有向量数据（用于重建索引）
   */
  async clear(): Promise<void> {
    if (this.table) {
      await this.db.dropTable(TABLE_NAME);
      this.table = null;
      this.logger.log('向量表已清空');
    }
  }
}
