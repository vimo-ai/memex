import { Controller, Get, Post, Query, Logger, Delete } from '@nestjs/common';
import { HybridSearchService, HybridSearchResult } from '../../application/services/hybrid-search.service';
import { VectorIndexerService } from '../../application/services/vector-indexer.service';
import { VectorLanceDbRepository } from '../../infrastructure/lancedb';

/**
 * 语义搜索查询参数
 */
interface SemanticSearchQuery {
  /** 搜索关键词 */
  q: string;
  /** 返回数量 */
  limit?: string;
  /** 项目 ID 过滤 */
  projectId?: string;
  /** 搜索模式：fts | vector | hybrid */
  mode?: 'fts' | 'vector' | 'hybrid';
  /** 开始时间（ISO 格式） */
  startDate?: string;
  /** 结束时间（ISO 格式） */
  endDate?: string;
}

/**
 * 语义搜索控制器
 *
 * 提供混合检索 API（FTS + 向量搜索）
 */
@Controller('search')
export class SemanticSearchController {
  private readonly logger = new Logger(SemanticSearchController.name);

  constructor(
    private readonly hybridSearchService: HybridSearchService,
    private readonly vectorIndexerService: VectorIndexerService,
  ) {}

  /**
   * 语义搜索
   *
   * GET /api/search/semantic?q=xxx&limit=20&mode=hybrid&startDate=2024-01-01&endDate=2024-12-31
   */
  @Get('semantic')
  async semanticSearch(@Query() query: SemanticSearchQuery): Promise<{
    results: HybridSearchResult[];
    total: number;
    mode: string;
  }> {
    const { q, limit = '20', projectId, mode = 'hybrid', startDate, endDate } = query;

    if (!q || q.trim().length === 0) {
      return { results: [], total: 0, mode };
    }

    const results = await this.hybridSearchService.search({
      query: q.trim(),
      limit: parseInt(limit, 10),
      projectId: projectId ? parseInt(projectId, 10) : undefined,
      mode,
      startDate,
      endDate,
    });

    return {
      results,
      total: results.length,
      mode,
    };
  }

  /**
   * 检查语义搜索是否可用
   *
   * GET /api/search/semantic/status
   */
  @Get('semantic/status')
  async getStatus(): Promise<{
    available: boolean;
    stats: {
      totalMessages: number;
      indexedMessages: number;
      pendingMessages: number;
      isIndexing: boolean;
      ollamaAvailable: boolean;
    };
  }> {
    const available = await this.hybridSearchService.isSemanticSearchAvailable();
    const stats = await this.vectorIndexerService.getStats();

    return {
      available,
      stats,
    };
  }
}

/**
 * Embedding 索引控制器
 *
 * 提供向量索引管理 API
 */
@Controller('embedding')
export class EmbeddingController {
  private readonly logger = new Logger(EmbeddingController.name);

  constructor(
    private readonly vectorIndexerService: VectorIndexerService,
    private readonly vectorRepository: VectorLanceDbRepository,
  ) {}

  /**
   * 全量构建向量索引
   *
   * POST /api/embedding/build
   */
  @Post('build')
  async buildIndex(): Promise<{
    success: boolean;
    processed: number;
    elapsed: number;
  }> {
    this.logger.log('收到全量索引构建请求');

    try {
      const result = await this.vectorIndexerService.buildFull();
      return {
        success: true,
        processed: result.processed,
        elapsed: result.elapsed,
      };
    } catch (error) {
      this.logger.error(`全量索引构建失败: ${error}`);
      throw error;
    }
  }

  /**
   * 触发增量索引
   *
   * POST /api/embedding/trigger
   */
  @Post('trigger')
  async triggerIncremental(): Promise<{
    success: boolean;
    processed: number;
    elapsed: number;
  }> {
    this.logger.log('收到增量索引触发请求');

    const result = await this.vectorIndexerService.trigger();
    return {
      success: true,
      processed: result.processed,
      elapsed: result.elapsed,
    };
  }

  /**
   * 获取索引状态
   *
   * GET /api/embedding/stats
   */
  @Get('stats')
  async getStats(): Promise<{
    totalMessages: number;
    indexedMessages: number;
    pendingMessages: number;
    failedMessages: number;
    isIndexing: boolean;
    ollamaAvailable: boolean;
    progress: number;
  }> {
    const stats = await this.vectorIndexerService.getStats();
    const progress =
      stats.totalMessages > 0 ? Math.round((stats.indexedMessages / stats.totalMessages) * 100) : 0;

    return {
      ...stats,
      progress,
    };
  }

  /**
   * 获取失败记录
   *
   * GET /api/embedding/failed
   */
  @Get('failed')
  async getFailedRecords(): Promise<{
    count: number;
    records: Array<{
      messageId: number;
      error: string;
      timestamp: Date;
    }>;
  }> {
    const records = this.vectorIndexerService.getFailedRecords();
    return {
      count: records.length,
      records: records.slice(0, 100), // 只返回前 100 条
    };
  }

  /**
   * 清空向量索引
   *
   * DELETE /api/embedding/clear
   *
   * 用于重建索引前清空旧数据（例如 schema 变更后）
   */
  @Delete('clear')
  async clearIndex(): Promise<{
    success: boolean;
    message: string;
  }> {
    this.logger.warn('收到清空向量索引请求');

    try {
      await this.vectorRepository.clear();
      this.vectorIndexerService.clearFailedRecords(); // 同时清空失败记录
      this.logger.log('向量索引已清空');
      return {
        success: true,
        message: '向量索引已清空，可以执行重建',
      };
    } catch (error) {
      this.logger.error(`清空向量索引失败: ${error}`);
      throw error;
    }
  }
}
