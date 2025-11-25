import { Injectable, Inject, Logger } from '@nestjs/common';
import {
  ISessionRepository,
  SESSION_REPOSITORY,
  SearchResult,
} from '../../domain/repositories/session.repository.interface';
import {
  IProjectRepository,
  PROJECT_REPOSITORY,
} from '../../domain/repositories/project.repository.interface';
import { VectorLanceDbRepository, VectorSearchResult } from '../../infrastructure/lancedb';
import { EmbeddingService } from './embedding.service';

/** RRF 融合常数，标准值为 60 */
const RRF_K = 60;

/**
 * 混合搜索结果
 */
export interface HybridSearchResult {
  /** 消息 ID */
  messageId: number;
  /** 消息 UUID */
  uuid: string;
  /** 消息内容（优先返回完整内容，否则返回分片内容） */
  content: string;
  /** 消息类型 */
  messageType: string;
  /** 会话 ID */
  sessionId: string;
  /** 项目 ID */
  projectId: number;
  /** 项目名称 */
  projectName: string;
  /** 消息时间戳 */
  timestamp: string;
  /** 匹配片段（FTS 高亮或向量匹配的 chunk） */
  snippet?: string;
  /** RRF 融合得分 */
  score: number;
  /** 来源标记 */
  sources: {
    fts: boolean;
    vector: boolean;
  };
  /** FTS 排名（如果来自 FTS） */
  ftsRank?: number;
  /** 向量相似度（如果来自向量搜索） */
  vectorSimilarity?: number;
  /** 匹配的分片索引（如果来自向量搜索） */
  chunkIndex?: number;
  /** 分片类型（如果来自向量搜索） */
  chunkType?: 'code' | 'text';
}

/**
 * 混合搜索选项
 */
export interface HybridSearchOptions {
  /** 搜索查询 */
  query: string;
  /** 返回数量 */
  limit?: number;
  /** 项目 ID 过滤 */
  projectId?: number;
  /** 搜索模式：fts | vector | hybrid */
  mode?: 'fts' | 'vector' | 'hybrid';
  /** 开始时间（ISO 格式字符串，可选） */
  startDate?: string;
  /** 结束时间（ISO 格式字符串，可选） */
  endDate?: string;
}

/**
 * 混合检索服务
 *
 * 职责：
 * - 同时执行 FTS 和向量搜索
 * - 使用 RRF (Reciprocal Rank Fusion) 融合排序
 * - 返回统一的搜索结果
 */
@Injectable()
export class HybridSearchService {
  private readonly logger = new Logger(HybridSearchService.name);

  constructor(
    @Inject(SESSION_REPOSITORY)
    private readonly sessionRepository: ISessionRepository,
    @Inject(PROJECT_REPOSITORY)
    private readonly projectRepository: IProjectRepository,
    private readonly vectorRepository: VectorLanceDbRepository,
    private readonly embeddingService: EmbeddingService,
  ) {}

  /**
   * 混合搜索
   */
  async search(options: HybridSearchOptions): Promise<HybridSearchResult[]> {
    const { query, limit = 20, projectId, mode = 'hybrid', startDate, endDate } = options;
    this.logger.log(`[搜索入口] query="${query}", mode=${mode}, limit=${limit}`);

    // 根据模式执行搜索
    let ftsResults: SearchResult[] = [];
    let vectorResults: VectorSearchResult[] = [];

    if (mode === 'fts' || mode === 'hybrid') {
      ftsResults = this.sessionRepository.searchMessages(query, limit * 2, startDate, endDate, projectId);
      this.logger.debug(`FTS 返回 ${ftsResults.length} 条结果`);
    }

    if (mode === 'vector' || mode === 'hybrid') {
      try {
        this.logger.log(`开始向量搜索: query="${query}"`);
        const queryVector = await this.embeddingService.embed(query);
        this.logger.log(`embedding 生成成功，维度: ${queryVector.length}`);
        vectorResults = await this.vectorRepository.search(queryVector, limit * 2, {
          projectId: projectId?.toString(),
          startDate,
          endDate,
        });
        this.logger.log(`向量搜索返回 ${vectorResults.length} 条结果`);
      } catch (error) {
        this.logger.warn(`向量搜索失败，降级为纯 FTS: ${error}`);
      }
    }

    // 如果两边都没结果
    if (ftsResults.length === 0 && vectorResults.length === 0) {
      return [];
    }

    // RRF 融合
    const fusedResults = this.rrfFusion(ftsResults, vectorResults, projectId);

    // 返回 top N
    return fusedResults.slice(0, limit);
  }

  /**
   * RRF (Reciprocal Rank Fusion) 排序融合
   *
   * 公式: score(d) = Σ 1/(k + rank_i(d))
   * k = 60 (标准值)
   *
   * 注意：向量搜索返回的是分片级别的结果，需要聚合到消息级别
   */
  private rrfFusion(
    ftsResults: SearchResult[],
    vectorResults: VectorSearchResult[],
    projectIdFilter?: number,
  ): HybridSearchResult[] {
    // 用 messageId 作为 key 聚合结果
    const scoreMap = new Map<
      number,
      {
        messageId: number;
        uuid: string;
        content: string;
        messageType: string;
        sessionId: string;
        projectId: number;
        projectName: string;
        timestamp: string;
        snippet?: string;
        score: number;
        fts: boolean;
        vector: boolean;
        ftsRank?: number;
        vectorSimilarity?: number;
        chunkIndex?: number;
        chunkType?: 'code' | 'text';
      }
    >();

    // 处理 FTS 结果（已在 SQL 层过滤，无需再次过滤）
    ftsResults.forEach((result, index) => {
      const messageId = result.message.id;
      if (!messageId) return;

      // 获取项目信息
      const session = this.sessionRepository.findSessionById(result.message.sessionId);
      if (!session) return;

      const project = this.projectRepository.findById(session.projectId);
      const rank = index + 1;
      const rrfScore = 1 / (RRF_K + rank);

      const existing = scoreMap.get(messageId);
      if (existing) {
        existing.score += rrfScore;
        existing.fts = true;
        existing.ftsRank = rank;
        existing.snippet = result.snippet;
      } else {
        scoreMap.set(messageId, {
          messageId,
          uuid: result.message.uuid,
          content: result.message.content,
          messageType: result.message.type,
          sessionId: result.message.sessionId,
          projectId: session.projectId,
          projectName: project?.name || '未知项目',
          timestamp: result.message.timestamp?.toISOString() || new Date().toISOString(),
          snippet: result.snippet,
          score: rrfScore,
          fts: true,
          vector: false,
          ftsRank: rank,
        });
      }
    });

    // 处理向量搜索结果（已在 LanceDB 层过滤，无需再次过滤）
    vectorResults.forEach((result, index) => {
      const messageId = result.messageId;

      const rank = index + 1;
      const rrfScore = 1 / (RRF_K + rank);

      const existing = scoreMap.get(messageId);
      if (existing) {
        existing.score += rrfScore;
        existing.vector = true;
        existing.vectorSimilarity = result.similarity;
        // 如果向量相似度更高，更新 snippet 为匹配的 chunk 内容
        if (!existing.vectorSimilarity || result.similarity > existing.vectorSimilarity) {
          existing.vectorSimilarity = result.similarity;
          existing.snippet = result.content; // 使用匹配的 chunk 作为 snippet
        }
      } else {
        // 获取项目名称
        const project = this.projectRepository.findById(Number(result.projectId));

        scoreMap.set(messageId, {
          messageId,
          uuid: result.uuid,
          // 优先使用完整内容，否则使用分片内容
          content: result.fullContent || result.content,
          messageType: result.messageType,
          sessionId: result.sessionId,
          projectId: Number(result.projectId),
          projectName: project?.name || '未知项目',
          timestamp: result.timestamp,
          snippet: result.content, // chunk 内容作为 snippet
          score: rrfScore,
          fts: false,
          vector: true,
          vectorSimilarity: result.similarity,
          chunkIndex: result.chunkIndex,
          chunkType: result.chunkType,
        });
      }
    });

    // 按 RRF 得分排序
    const sorted = Array.from(scoreMap.values()).sort((a, b) => b.score - a.score);

    // 转换为最终结果格式
    return sorted.map((item) => ({
      messageId: item.messageId,
      uuid: item.uuid,
      content: item.content,
      messageType: item.messageType,
      sessionId: item.sessionId,
      projectId: item.projectId,
      projectName: item.projectName,
      timestamp: item.timestamp,
      snippet: item.snippet,
      score: item.score,
      sources: {
        fts: item.fts,
        vector: item.vector,
      },
      ftsRank: item.ftsRank,
      vectorSimilarity: item.vectorSimilarity,
      chunkIndex: item.chunkIndex,
      chunkType: item.chunkType,
    }));
  }

  /**
   * 检查语义搜索是否可用
   */
  async isSemanticSearchAvailable(): Promise<boolean> {
    const [ollamaOk, vectorStats] = await Promise.all([
      this.embeddingService.isAvailable(),
      this.vectorRepository.getStats(),
    ]);

    return ollamaOk && vectorStats.tableExists && vectorStats.totalRecords > 0;
  }
}
