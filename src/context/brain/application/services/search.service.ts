import { Inject, Injectable } from '@nestjs/common';
import {
  ISessionRepository,
  SESSION_REPOSITORY,
  SearchResult,
} from '../../domain/repositories/session.repository.interface';
import {
  IProjectRepository,
  PROJECT_REPOSITORY,
} from '../../domain/repositories/project.repository.interface';
import { MessageEntity } from '../../domain/entities/message.entity';

/**
 * 搜索选项
 */
export interface SearchOptions {
  /** 搜索关键词 */
  query: string;
  /** 项目 ID（可选，限定搜索范围） */
  projectId?: number;
  /** 返回数量限制 */
  limit?: number;
}

/**
 * 带上下文的搜索结果
 */
export interface SearchResultWithContext extends SearchResult {
  /** 项目 ID */
  projectId?: number;
  /** 项目名称 */
  projectName?: string;
}

/**
 * 搜索服务
 *
 * 封装 FTS5 全文搜索逻辑，提供高级搜索功能
 */
@Injectable()
export class SearchService {
  constructor(
    @Inject(SESSION_REPOSITORY)
    private readonly sessionRepository: ISessionRepository,
    @Inject(PROJECT_REPOSITORY)
    private readonly projectRepository: IProjectRepository,
  ) {}

  /**
   * 全文搜索消息
   *
   * @param options 搜索选项
   * @returns 搜索结果列表（包含项目信息）
   */
  search(options: SearchOptions): SearchResultWithContext[] {
    const { query, projectId, limit = 50 } = options;

    // 执行基础搜索
    let results = this.sessionRepository.searchMessages(query, limit);

    // 如果指定了项目 ID，过滤结果
    if (projectId !== undefined) {
      const sessions = this.sessionRepository.findSessionsByProjectId(projectId);
      const sessionIds = new Set(sessions.map((s) => s.id));

      results = results.filter((r) => sessionIds.has(r.message.sessionId));
    }

    // 为每个结果添加项目信息
    return results.map((result) => {
      const session = this.sessionRepository.findSessionById(result.message.sessionId);
      let projectName: string | undefined;
      let resultProjectId: number | undefined;

      if (session) {
        resultProjectId = session.projectId;
        const project = this.projectRepository.findById(session.projectId);
        projectName = project?.name;
      }

      return {
        ...result,
        projectId: resultProjectId,
        projectName,
      };
    });
  }

  /**
   * 搜索指定会话中的消息
   *
   * @param sessionId 会话 ID
   * @param query 搜索关键词
   * @param limit 返回数量限制
   */
  searchInSession(sessionId: string, query: string, limit: number = 50): SearchResult[] {
    const results = this.sessionRepository.searchMessages(query, limit * 2);

    // 过滤指定会话的结果
    return results.filter((r) => r.message.sessionId === sessionId).slice(0, limit);
  }

  /**
   * 获取建议的搜索关键词
   *
   * 基于消息内容提取高频词汇（简化实现）
   * 后续可以结合向量搜索提供更智能的建议
   */
  getSuggestions(prefix: string, limit: number = 10): string[] {
    // 简化实现：返回包含前缀的搜索结果
    // 实际生产环境可以使用更复杂的算法
    if (!prefix || prefix.length < 2) {
      return [];
    }

    const results = this.sessionRepository.searchMessages(prefix, limit);
    const words = new Set<string>();

    for (const result of results) {
      // 从内容中提取包含前缀的词
      const content = result.message.content.toLowerCase();
      const prefixLower = prefix.toLowerCase();

      // 简单的词提取（按空格和标点分割）
      const tokens = content.split(/[\s,.!?;:，。！？；：\n]+/);

      for (const token of tokens) {
        if (token.includes(prefixLower) && token.length <= 30) {
          words.add(token);
          if (words.size >= limit) break;
        }
      }

      if (words.size >= limit) break;
    }

    return Array.from(words);
  }
}
