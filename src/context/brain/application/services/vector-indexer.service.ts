import { Injectable, Inject, Logger, OnModuleInit } from '@nestjs/common';
import { Cron, CronExpression } from '@nestjs/schedule';
import { SQLITE_DB, SqliteDatabase } from '../../infrastructure/sqlite/sqlite.provider';
import { VectorLanceDbRepository, VectorRecord } from '../../infrastructure/lancedb';
import { EmbeddingService } from './embedding.service';
import { ChunkingService } from './chunking.service';
import { MessageType } from '../../domain/entities/message.entity';

/** 批量处理大小 */
const BATCH_SIZE = 50;

/** 累积文本长度阈值（保守设置，实测 2836 会崩溃，降到 2500） */
const ACCUMULATION_THRESHOLD = 2500;

/**
 * 向量索引构建服务
 *
 * 职责：
 * - 全量构建向量索引
 * - 增量构建（新消息）
 * - 定时任务触发
 */
/** 失败记录 */
interface FailedRecord {
  messageId: number;
  error: string;
  timestamp: Date;
}

@Injectable()
export class VectorIndexerService implements OnModuleInit {
  private readonly logger = new Logger(VectorIndexerService.name);
  private isIndexing = false;
  private failedRecords: FailedRecord[] = [];

  constructor(
    @Inject(SQLITE_DB)
    private readonly db: SqliteDatabase,
    private readonly vectorRepository: VectorLanceDbRepository,
    private readonly embeddingService: EmbeddingService,
    private readonly chunkingService: ChunkingService,
  ) {}

  async onModuleInit() {
    // 启动时检查 Ollama 可用性
    const available = await this.embeddingService.isAvailable();
    if (available) {
      this.logger.log('Ollama 服务可用，向量索引功能已启用');

      // 启动 10 秒后自动进行增量索引
      setTimeout(async () => {
        try {
          await this.buildIncremental();
        } catch (error) {
          this.logger.error(`启动时增量索引失败: ${error}`);
        }
      }, 10000);
    } else {
      this.logger.warn('Ollama 服务不可用，向量索引功能已禁用');
    }
  }

  /**
   * 定时增量构建（每小时）
   */
  @Cron(CronExpression.EVERY_HOUR)
  async scheduledBuild() {
    this.logger.log('定时任务触发增量索引构建');
    await this.buildIncremental();
  }

  /**
   * 全量构建向量索引
   *
   * 会清空现有索引，重新构建所有 assistant 消息的向量
   */
  async buildFull(): Promise<{ processed: number; elapsed: number }> {
    if (this.isIndexing) {
      throw new Error('索引构建正在进行中');
    }

    this.isIndexing = true;
    const startTime = Date.now();

    try {
      // 清空现有索引
      await this.vectorRepository.clear();
      this.logger.log('已清空现有向量索引');

      // 获取所有 assistant 消息
      const messages = this.getAllAssistantMessages();
      this.logger.log(`待索引消息数: ${messages.length}`);

      if (messages.length === 0) {
        return { processed: 0, elapsed: 0 };
      }

      // 分批处理
      let processed = 0;
      for (let i = 0; i < messages.length; i += BATCH_SIZE) {
        const batch = messages.slice(i, i + BATCH_SIZE);
        const batchSuccess = await this.processBatch(batch, messages.length, processed);
        processed += batchSuccess;
      }

      const elapsed = Date.now() - startTime;
      this.logger.log(`全量索引完成: ${processed} 条, 耗时 ${(elapsed / 1000).toFixed(1)}s`);

      return { processed, elapsed };
    } finally {
      this.isIndexing = false;
    }
  }

  /**
   * 增量构建向量索引
   *
   * 只处理尚未索引的消息
   */
  async buildIncremental(): Promise<{ processed: number; elapsed: number }> {
    if (this.isIndexing) {
      this.logger.debug('索引构建正在进行中，跳过');
      return { processed: 0, elapsed: 0 };
    }

    // 检查 Ollama 可用性
    const available = await this.embeddingService.isAvailable();
    if (!available) {
      this.logger.debug('Ollama 不可用，跳过增量索引');
      return { processed: 0, elapsed: 0 };
    }

    this.isIndexing = true;
    const startTime = Date.now();

    try {
      // 获取已索引的消息 ID
      const indexedIds = await this.vectorRepository.getIndexedMessageIds();
      this.logger.debug(`已索引消息数: ${indexedIds.size}`);

      // 获取所有 assistant 消息
      const allMessages = this.getAllAssistantMessages();

      // 过滤出未索引的消息
      const unindexedMessages = allMessages.filter((m) => !indexedIds.has(m.id));
      this.logger.debug(`待索引消息数: ${unindexedMessages.length}`);

      if (unindexedMessages.length === 0) {
        return { processed: 0, elapsed: 0 };
      }

      // 分批处理
      let processed = 0;
      for (let i = 0; i < unindexedMessages.length; i += BATCH_SIZE) {
        const batch = unindexedMessages.slice(i, i + BATCH_SIZE);
        const batchSuccess = await this.processBatch(batch, unindexedMessages.length, processed);
        processed += batchSuccess;
      }

      const elapsed = Date.now() - startTime;
      if (processed > 0) {
        this.logger.log(`增量索引完成: ${processed} 条, 耗时 ${(elapsed / 1000).toFixed(1)}s`);
      }

      return { processed, elapsed };
    } finally {
      this.isIndexing = false;
    }
  }

  /**
   * 触发增量构建（供 API 调用）
   */
  async trigger(): Promise<{ processed: number; elapsed: number }> {
    return this.buildIncremental();
  }

  /**
   * 获取索引状态
   */
  async getStats(): Promise<{
    totalMessages: number;
    indexedMessages: number;
    pendingMessages: number;
    failedMessages: number;
    isIndexing: boolean;
    ollamaAvailable: boolean;
  }> {
    const totalMessages = this.countAssistantMessages();
    const vectorStats = await this.vectorRepository.getStats();
    const ollamaAvailable = await this.embeddingService.isAvailable();

    return {
      totalMessages,
      indexedMessages: vectorStats.totalRecords,
      pendingMessages: totalMessages - vectorStats.totalRecords,
      failedMessages: this.failedRecords.length,
      isIndexing: this.isIndexing,
      ollamaAvailable,
    };
  }

  /**
   * 处理一批消息（并发处理，事务性提交）
   * 每条消息的所有 chunk 全部成功才会被记录
   *
   * @param messages 待处理消息
   * @param totalMessages 总消息数（用于显示进度）
   * @param alreadyProcessed 已处理消息数（用于显示进度）
   */
  private async processBatch(
    messages: Array<{
      id: number;
      uuid: string;
      sessionId: string;
      projectId: number;
      content: string;
      type: string;
      timestamp?: string;
    }>,
    totalMessages?: number,
    alreadyProcessed?: number,
  ): Promise<number> {
    const successRecords: VectorRecord[] = [];
    let processedCount = 0;
    let failedCount = 0;

    // 串行处理消息（Ollama embedding worker 不稳定，避免并发导致 EOF）
    const CONCURRENCY = 1;

    // 追踪累积文本长度（智能卸载策略）
    let accumulatedLength = 0;

    for (let i = 0; i < messages.length; i += CONCURRENCY) {
      const batch = messages.slice(i, i + CONCURRENCY);

      // 【临时禁用卸载策略，测试性能】
      // // 在处理每条消息前判断是否需要卸载
      // for (const msg of batch) {
      //   if (accumulatedLength + msg.content.length > ACCUMULATION_THRESHOLD) {
      //     this.logger.log(
      //       `累积 ${accumulatedLength} + 当前 ${msg.content.length} = ${accumulatedLength + msg.content.length} 将超阈值，先卸载`,
      //     );
      //     await this.embeddingService.unloadModel();
      //     accumulatedLength = 0;
      //   }
      // }

      // 并发处理这批消息
      const results = await Promise.allSettled(
        batch.map((msg) => this.processOneMessage(msg)),
      );

      // 收集成功的记录
      for (let index = 0; index < results.length; index++) {
        const result = results[index];
        if (result.status === 'fulfilled' && result.value.length > 0) {
          successRecords.push(...result.value);
          processedCount++;
          // 处理成功后累加长度
          accumulatedLength += batch[index].content.length;
        } else {
          // 记录失败详情
          if (this.failedRecords.length < 1000) {
            const msg = batch[index];
            let errorMsg: string;
            if (result.status === 'rejected') {
              errorMsg = String(result.reason);
            } else {
              // fulfilled 但返回空数组（可能是空内容或 chunk 失败）
              errorMsg = `空结果 (content.length=${msg.content?.length || 0})`;
            }
            this.failedRecords.push({
              messageId: msg.id,
              error: errorMsg,
              timestamp: new Date(),
            });
            // 打印失败详情便于调试
            this.logger.warn(
              `消息 ${msg.id} 处理失败: ${errorMsg}, sessionId=${msg.sessionId}`,
            );
          }
          failedCount++;
        }
      }
    }

    // 批量插入
    if (successRecords.length > 0) {
      await this.vectorRepository.upsertBatch(successRecords);
    }

    // 打印合并后的进度日志
    if (totalMessages && totalMessages > 0) {
      const currentProcessed = (alreadyProcessed || 0) + processedCount;
      const percentage = ((currentProcessed / totalMessages) * 100).toFixed(1);
      this.logger.log(
        `进度: ${currentProcessed}/${totalMessages} (${percentage}%), 本批成功: ${processedCount}, 失败: ${failedCount}`,
      );
    } else {
      this.logger.log(`进度: ${processedCount}/${messages.length} 消息, 成功: ${processedCount}, 失败: ${failedCount}`);
    }

    return processedCount;
  }

  /**
   * 处理单条消息（事务性：所有 chunk 成功才返回记录，否则返回空数组）
   */
  private async processOneMessage(msg: {
    id: number;
    uuid: string;
    sessionId: string;
    projectId: number;
    content: string;
    type: string;
    timestamp?: string;
  }): Promise<VectorRecord[]> {
    const chunks = this.chunkingService.chunk(msg.content);
    const records: VectorRecord[] = [];

    // 逐个 chunk 处理（消息内部串行，避免单消息占用太多并发）
    for (const chunk of chunks) {
      const embedding = await this.embeddingService.embed(chunk.content);
      records.push({
        messageId: msg.id,
        chunkIndex: chunk.index,
        uuid: msg.uuid,
        sessionId: msg.sessionId,
        projectId: msg.projectId.toString(),
        content: chunk.content,
        fullContent: msg.content,
        messageType: msg.type,
        chunkType: chunk.type,
        // timestamp: msg.timestamp,  // MVP 阶段暂不使用
        vector: embedding,
      });
    }

    // 全部成功才返回记录
    return records;
  }

  /**
   * 获取失败记录
   */
  getFailedRecords(): FailedRecord[] {
    return this.failedRecords;
  }

  /**
   * 清空失败记录
   */
  clearFailedRecords(): void {
    this.failedRecords = [];
  }

  /**
   * 获取所有 assistant 消息（带项目 ID 和 timestamp）
   */
  private getAllAssistantMessages(): Array<{
    id: number;
    uuid: string;
    sessionId: string;
    projectId: number;
    content: string;
    type: string;
    timestamp?: string;
  }> {
    const stmt = this.db.prepare(`
      SELECT
        m.id,
        m.uuid,
        m.session_id as sessionId,
        s.project_id as projectId,
        m.content,
        m.type,
        m.timestamp
      FROM messages m
      JOIN sessions s ON m.session_id = s.id
      WHERE m.type = ?
      ORDER BY m.id
    `);

    return stmt.all(MessageType.ASSISTANT) as Array<{
      id: number;
      uuid: string;
      sessionId: string;
      projectId: number;
      content: string;
      type: string;
      timestamp?: string;
    }>;
  }

  /**
   * 统计 assistant 消息数量
   */
  private countAssistantMessages(): number {
    const stmt = this.db.prepare(`
      SELECT COUNT(*) as count FROM messages WHERE type = ?
    `);
    const result = stmt.get(MessageType.ASSISTANT) as { count: number };
    return result.count;
  }
}
