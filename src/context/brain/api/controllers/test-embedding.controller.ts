import { Controller, Post, Logger, Inject, Body } from '@nestjs/common';
import { EmbeddingService } from '../../application/services/embedding.service';
import { SQLITE_DB, SqliteDatabase } from '../../infrastructure/sqlite/sqlite.provider';
import { MessageType } from '../../domain/entities/message.entity';
import { VectorLanceDbRepository } from '../../infrastructure/lancedb';

/** 累积文本长度阈值（与 vector-indexer.service.ts 保持一致） */
const ACCUMULATION_THRESHOLD = 2500;

/**
 * Embedding 测试控制器
 *
 * 用于测试 Ollama embedding 崩溃问题
 */
@Controller('test')
export class TestEmbeddingController {
  private readonly logger = new Logger(TestEmbeddingController.name);

  constructor(
    private readonly embeddingService: EmbeddingService,
    @Inject(SQLITE_DB)
    private readonly db: SqliteDatabase,
    private readonly vectorRepository: VectorLanceDbRepository,
  ) {}

  /**
   * 单独测试消息 ID 84799
   *
   * POST /api/test/embedding-single
   */
  @Post('embedding-single')
  async testSingleMessage(): Promise<{
    success: boolean;
    messageId: number;
    textLength: number;
    error?: string;
    vectorDim?: number;
  }> {
    const messageId = 84799;
    this.logger.log(`========== 测试单条消息 ID: ${messageId} ==========`);

    // 从数据库读取这条消息
    const stmt = this.db.prepare(`SELECT id, content FROM messages WHERE id = ?`);
    const msg = stmt.get(messageId) as { id: number; content: string } | undefined;

    if (!msg) {
      return {
        success: false,
        messageId,
        textLength: 0,
        error: '消息不存在',
      };
    }

    this.logger.log(`消息长度: ${msg.content.length}`);
    this.logger.log(`完整内容:\n${msg.content}`);
    this.logger.log(`开始生成 embedding...`);

    try {
      const startTime = Date.now();
      const vector = await this.embeddingService.embed(msg.content);
      const duration = Date.now() - startTime;

      this.logger.log(`✅ 成功！耗时: ${duration}ms, 向量维度: ${vector.length}`);

      return {
        success: true,
        messageId,
        textLength: msg.content.length,
        vectorDim: vector.length,
      };
    } catch (error) {
      this.logger.error(`❌ 失败！错误: ${error.message}`);

      return {
        success: false,
        messageId,
        textLength: msg.content.length,
        error: error.message,
      };
    }
  }

  /**
   * 并发测试：生成 50 个 embedding，记录详细日志
   *
   * POST /api/test/embedding-5
   */
  @Post('embedding-5')
  async testFiveEmbeddings(): Promise<{
    success: boolean;
    results: Array<{
      index: number;
      textLength: number;
      textPreview: string;
      success: boolean;
      error?: string;
      vectorDim?: number;
    }>;
  }> {
    this.logger.log('========== 开始测试：并发生成 50 个 embedding ==========');

    // 准备 50 个测试文本（使用简单文本，避免数据库查询）
    const baseTexts = [
      'This is a simple test message number {i}.',
      'Here is the second test message with some different content for item {i}.',
      'The third message contains a bit more text to test various lengths and see how the model handles it. Message {i}.',
      'Message number {i} includes some special characters: @#$%^&*() and also some numbers: 12345.',
      'Finally, message {i} is here to complete our test. It has a reasonable length and normal content.',
      'Another variant for message {i} with markdown: **bold** _italic_ `code` and [link](url).',
      'Testing unicode characters in message {i}: 中文测试 émojis 🚀 ñoño',
      'Long text message {i}: ' + 'a'.repeat(500) + ' end of long text.',
      'JSON-like content in message {i}: {"key": "value", "number": 123, "array": [1,2,3]}',
      'Code snippet in message {i}: function test() { return "hello world"; }',
    ];

    const testTexts: string[] = [];
    for (let i = 0; i < 50; i++) {
      const template = baseTexts[i % baseTexts.length];
      testTexts.push(template.replace(/\{i\}/g, String(i + 1)));
    }

    const results: Array<{
      index: number;
      textLength: number;
      textPreview: string;
      success: boolean;
      error?: string;
      vectorDim?: number;
    }> = [];

    // 并发发送 5 个请求
    const promises = testTexts.map(async (text, index) => {
      const textPreview = text.slice(0, 100);
      this.logger.log(
        `[${index + 1}] 开始处理 | 长度: ${text.length} | 内容: "${textPreview}"`,
      );

      try {
        const startTime = Date.now();
        const vector = await this.embeddingService.embed(text);
        const duration = Date.now() - startTime;

        this.logger.log(
          `[${index + 1}] ✅ 成功 | 耗时: ${duration}ms | 向量维度: ${vector.length}`,
        );

        return {
          index: index + 1,
          textLength: text.length,
          textPreview,
          success: true,
          vectorDim: vector.length,
        };
      } catch (error) {
        this.logger.error(`[${index + 1}] ❌ 失败 | 错误: ${error.message}`);
        this.logger.error(`[${index + 1}] 完整文本: "${text}"`);

        return {
          index: index + 1,
          textLength: text.length,
          textPreview,
          success: false,
          error: error.message,
        };
      }
    });

    // 等待所有请求完成
    const settled = await Promise.allSettled(promises);

    // 收集结果
    for (const result of settled) {
      if (result.status === 'fulfilled') {
        results.push(result.value);
      } else {
        this.logger.error(`Promise rejected: ${result.reason}`);
        results.push({
          index: results.length + 1,
          textLength: 0,
          textPreview: 'Promise rejected',
          success: false,
          error: String(result.reason),
        });
      }
    }

    const successCount = results.filter((r) => r.success).length;
    this.logger.log(`========== 测试完成：${successCount}/50 成功 ==========`);

    return {
      success: successCount === 50,
      results,
    };
  }

  /**
   * 真实数据测试：自动分批测试所有消息，遇到第一个失败立即停止
   *
   * POST /api/test/embedding-real
   * Body (可选):
   *   - startFrom: 从第 N 条开始测试
   *   - skipIndexed: 是否跳过已索引的消息（默认 false）
   */
  @Post('embedding-real')
  async testRealMessages(@Body() body?: { startFrom?: number; skipIndexed?: boolean }): Promise<{
    success: boolean;
    totalMessagesInDb: number;
    testedCount: number;
    failedMessageId?: number;
    failedContent?: string;
    error?: string;
  }> {
    const startFrom = body?.startFrom || 0;
    const skipIndexed = body?.skipIndexed || false;

    this.logger.log(
      `========== 开始测试：自动分批测试所有真实消息 (从第 ${startFrom + 1} 条开始${skipIndexed ? ', 跳过已索引' : ''}) ==========`,
    );

    // 获取消息总数
    const countStmt = this.db.prepare(`
      SELECT COUNT(*) as count FROM messages WHERE type = ?
    `);
    const { count: totalCount } = countStmt.get(MessageType.ASSISTANT) as { count: number };

    this.logger.log(`数据库中共有 ${totalCount} 条 assistant 消息`);

    if (totalCount === 0) {
      return {
        success: false,
        totalMessagesInDb: 0,
        testedCount: 0,
        error: '数据库中没有消息',
      };
    }

    // 获取所有消息 ID 和内容
    const stmt = this.db.prepare(`
      SELECT id, content
      FROM messages
      WHERE type = ?
      ORDER BY id
    `);

    let allMessages = stmt.all(MessageType.ASSISTANT) as Array<{
      id: number;
      content: string;
    }>;

    // 如果需要跳过已索引的消息
    if (skipIndexed) {
      const indexedIds = await this.vectorRepository.getIndexedMessageIds();
      this.logger.log(`已索引消息数: ${indexedIds.size}`);
      allMessages = allMessages.filter(m => !indexedIds.has(m.id));
      this.logger.log(`过滤后待索引消息数: ${allMessages.length}`);
    }

    // 从指定位置开始
    const messages = allMessages.slice(startFrom);
    this.logger.log(`跳过前 ${startFrom} 条，从第 ${startFrom + 1} 条开始测试，剩余 ${messages.length} 条`);

    // 追踪累积文本长度（智能卸载策略）
    let accumulatedLength = 0;

    // 串行处理每条消息，遇到失败立即停止
    for (let i = 0; i < messages.length; i++) {
      const actualIndex = startFrom + i; // 真实的索引位置
      const msg = messages[i];
      const textPreview = msg.content.slice(0, 100).replace(/\n/g, ' ');

      // 在处理前判断是否需要卸载
      if (accumulatedLength + msg.content.length > ACCUMULATION_THRESHOLD) {
        this.logger.log(
          `[${actualIndex + 1}/${totalCount}] 累积: ${accumulatedLength}, 当前: ${msg.content.length}, 将达: ${accumulatedLength + msg.content.length} → 先卸载`,
        );
        this.logger.log(`⚙️  卸载模型释放内存...`);
        await this.embeddingService.unloadModel();
        accumulatedLength = 0;
      }

      this.logger.log(
        `[${actualIndex + 1}/${totalCount}] 测试消息 ID: ${msg.id} | 长度: ${msg.content.length} | 累积: ${accumulatedLength} → ${accumulatedLength + msg.content.length}`,
      );

      try {
        const startTime = Date.now();
        const vector = await this.embeddingService.embed(msg.content);
        const duration = Date.now() - startTime;

        // 处理成功后累加长度
        accumulatedLength += msg.content.length;

        this.logger.log(
          `[${actualIndex + 1}/${totalCount}] ✅ 成功 | 耗时: ${duration}ms | 向量维度: ${vector.length}`,
        );
      } catch (error) {
        // 遇到第一个失败，打印完整内容并停止
        this.logger.error(`\n${'='.repeat(80)}`);
        this.logger.error(`❌❌❌ 发现问题消息！位置: [${actualIndex + 1}/${totalCount}]`);
        this.logger.error(`消息 ID: ${msg.id}`);
        this.logger.error(`错误信息: ${error.message}`);
        this.logger.error(`当前消息长度: ${msg.content.length}`);
        this.logger.error(`累积长度: ${accumulatedLength}`);
        this.logger.error(`${'='.repeat(80)}`);
        this.logger.error(`完整内容:\n${msg.content}`);
        this.logger.error(`${'='.repeat(80)}\n`);

        return {
          success: false,
          totalMessagesInDb: totalCount,
          testedCount: actualIndex + 1,
          failedMessageId: msg.id,
          failedContent: msg.content,
          error: error.message,
        };
      }
    }

    // 全部成功
    this.logger.log(`\n${'='.repeat(80)}`);
    this.logger.log(
      `🎉 全部测试通过！从第 ${startFrom + 1} 条开始，${messages.length} 条消息全部成功生成 embedding`,
    );
    this.logger.log(`${'='.repeat(80)}\n`);

    return {
      success: true,
      totalMessagesInDb: totalCount,
      testedCount: messages.length,
    };
  }
}
