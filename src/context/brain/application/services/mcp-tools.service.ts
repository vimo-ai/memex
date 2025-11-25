import { Injectable, Inject, Logger } from '@nestjs/common';
import { MCPTool } from '../../mcp/decorators/mcp-tool.decorator';
import { HybridSearchService, HybridSearchResult } from './hybrid-search.service';
import {
  ISessionRepository,
  SESSION_REPOSITORY,
} from '../../domain/repositories/session.repository.interface';
import {
  IProjectRepository,
  PROJECT_REPOSITORY,
} from '../../domain/repositories/project.repository.interface';
import { SessionEntity } from '../../domain/entities/session.entity';
import { MessageEntity } from '../../domain/entities/message.entity';

/**
 * MCP 工具服务
 * 提供 4 个 MCP 工具用于访问历史会话
 */
@Injectable()
export class MCPToolsService {
  private readonly logger = new Logger(MCPToolsService.name);

  constructor(
    private readonly hybridSearchService: HybridSearchService,
    @Inject(SESSION_REPOSITORY)
    private readonly sessionRepository: ISessionRepository,
    @Inject(PROJECT_REPOSITORY)
    private readonly projectRepository: IProjectRepository,
  ) {}

  /**
   * 搜索历史对话
   */
  @MCPTool({
    name: 'search_history',
    description: '搜索 Claude Code 历史对话，支持全文搜索、向量语义搜索和混合搜索',
    inputSchema: {
      type: 'object',
      properties: {
        query: { type: 'string', description: '搜索关键词' },
        mode: {
          type: 'string',
          enum: ['fts', 'vector', 'hybrid'],
          description: '搜索模式：fts (全文搜索) / vector (语义搜索) / hybrid (混合搜索，默认)',
        },
        cwd: { type: 'string', description: '当前工作目录，用于匹配项目并过滤结果' },
        limit: { type: 'number', description: '返回数量，默认 10' },
      },
      required: ['query'],
    },
  })
  async searchHistory(
    query: string,
    mode?: 'fts' | 'vector' | 'hybrid',
    cwd?: string,
    limit?: number,
  ): Promise<{
    results: Array<{
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
      sources: { fts: boolean; vector: boolean };
      messageIndex?: number;
    }>;
    total: number;
  }> {
    // 如果提供了 cwd，尝试匹配项目
    let projectId: number | undefined;
    if (cwd) {
      const project = this.findProjectByCwd(cwd);
      if (project) {
        projectId = project.id;
        this.logger.log(`匹配到项目: ${project.name} (ID: ${projectId})`);
      } else {
        this.logger.log(`未匹配到项目，cwd: ${cwd}`);
      }
    }

    // 执行搜索
    const results = await this.hybridSearchService.search({
      query,
      mode: mode || 'hybrid',
      limit: limit || 10,
      projectId,
    });

    // 为每个结果添加 messageIndex
    const enrichedResults = await Promise.all(
      results.map(async (result) => {
        const messageIndex = await this.getMessageIndex(result.sessionId, result.uuid);
        return {
          ...result,
          messageIndex,
        };
      }),
    );

    return {
      results: enrichedResults,
      total: enrichedResults.length,
    };
  }

  /**
   * 获取会话详情
   */
  @MCPTool({
    name: 'get_session',
    description: '获取会话详情，支持分页和会话内搜索。返回会话基本信息和消息列表',
    inputSchema: {
      type: 'object',
      properties: {
        sessionId: { type: 'string', description: '会话 ID（完整 UUID 或前缀）' },
        offset: { type: 'number', description: '从第几条消息开始，默认 0' },
        limit: { type: 'number', description: '返回消息数量，默认 10' },
        search: {
          type: 'string',
          description: '会话内搜索关键词，自动定位到匹配位置并返回匹配消息',
        },
      },
      required: ['sessionId'],
    },
  })
  async getSession(
    sessionId: string,
    offset?: number,
    limit?: number,
    search?: string,
  ): Promise<{
    session: {
      id: string;
      projectId: number;
      projectName: string;
      createdAt: string;
      updatedAt: string;
      messageCount: number;
    };
    messages: Array<{
      id: number;
      uuid: string;
      type: string;
      content: string;
      timestamp: string;
      index: number;
    }>;
    pagination: {
      offset: number;
      limit: number;
      total: number;
    };
  }> {
    // 查找会话
    let session = this.sessionRepository.findSessionById(sessionId);

    // 如果找不到，尝试按前缀搜索
    if (!session) {
      const sessions = this.sessionRepository.searchSessionsByIdPrefix(sessionId, 1);
      if (sessions.length > 0) {
        session = sessions[0];
      } else {
        throw new Error(`Session not found: ${sessionId}`);
      }
    }

    // 获取项目信息
    const project = this.projectRepository.findById(session.projectId);

    // 获取所有消息
    const allMessages = this.sessionRepository.findMessagesBySessionId(session.id);

    let messages: MessageEntity[];
    let actualOffset = offset || 0;

    // 如果有 search 参数，查找匹配的消息并自动定位
    if (search) {
      const searchLower = search.toLowerCase();
      const matchIndex = allMessages.findIndex((msg) =>
        msg.content.toLowerCase().includes(searchLower),
      );

      if (matchIndex !== -1) {
        // 找到匹配，从匹配位置开始
        actualOffset = matchIndex;
      }
    }

    // 分页
    const limitValue = limit || 10;
    messages = allMessages.slice(actualOffset, actualOffset + limitValue);

    return {
      session: {
        id: session.id,
        projectId: session.projectId,
        projectName: project?.name || '未知项目',
        createdAt: session.createdAt?.toISOString() || new Date().toISOString(),
        updatedAt: session.updatedAt?.toISOString() || new Date().toISOString(),
        messageCount: allMessages.length,
      },
      messages: messages.map((msg, idx) => ({
        id: msg.id!,
        uuid: msg.uuid,
        type: msg.type,
        content: msg.content,
        timestamp: msg.timestamp?.toISOString() || '',
        index: actualOffset + idx,
      })),
      pagination: {
        offset: actualOffset,
        limit: limitValue,
        total: allMessages.length,
      },
    };
  }

  /**
   * 获取最近的会话列表
   */
  @MCPTool({
    name: 'get_recent_sessions',
    description: '获取最近的会话列表，按更新时间倒序排列。可选择性过滤指定项目的会话',
    inputSchema: {
      type: 'object',
      properties: {
        cwd: { type: 'string', description: '当前工作目录，用于匹配项目并过滤结果' },
        limit: { type: 'number', description: '返回数量，默认 5' },
      },
    },
  })
  async getRecentSessions(
    cwd?: string,
    limit?: number,
  ): Promise<{
    sessions: Array<{
      id: string;
      projectId: number;
      projectName: string;
      createdAt: string;
      updatedAt: string;
      messageCount: number;
      lastMessage?: {
        type: string;
        content: string;
        timestamp: string;
      };
    }>;
    total: number;
  }> {
    // 如果提供了 cwd，尝试匹配项目
    let projectId: number | undefined;
    if (cwd) {
      const project = this.findProjectByCwd(cwd);
      if (project) {
        projectId = project.id;
        this.logger.log(`匹配到项目: ${project.name} (ID: ${projectId})`);
      }
    }

    // 获取所有会话
    let allSessions = this.sessionRepository.findAllSessions();

    // 如果有 projectId，过滤
    if (projectId !== undefined) {
      allSessions = allSessions.filter((s) => s.projectId === projectId);
    }

    // 按更新时间倒序排序
    allSessions.sort((a, b) => {
      const aTime = a.updatedAt?.getTime() || 0;
      const bTime = b.updatedAt?.getTime() || 0;
      return bTime - aTime;
    });

    // 取前 N 个
    const limitValue = limit || 5;
    const sessions = allSessions.slice(0, limitValue);

    // 构造返回结果
    const results = sessions.map((session) => {
      const project = this.projectRepository.findById(session.projectId);
      const messages = this.sessionRepository.findMessagesBySessionId(session.id);
      const lastMessage = messages.length > 0 ? messages[messages.length - 1] : undefined;

      return {
        id: session.id,
        projectId: session.projectId,
        projectName: project?.name || '未知项目',
        createdAt: session.createdAt?.toISOString() || new Date().toISOString(),
        updatedAt: session.updatedAt?.toISOString() || new Date().toISOString(),
        messageCount: messages.length,
        lastMessage: lastMessage
          ? {
              type: lastMessage.type,
              content: lastMessage.content.slice(0, 200), // 限制长度
              timestamp: lastMessage.timestamp?.toISOString() || '',
            }
          : undefined,
      };
    });

    return {
      sessions: results,
      total: allSessions.length,
    };
  }

  /**
   * 列出所有项目
   */
  @MCPTool({
    name: 'list_projects',
    description: '列出所有项目，包括项目名称、路径、会话数量等信息',
    inputSchema: {
      type: 'object',
      properties: {},
    },
  })
  async listProjects(): Promise<{
    projects: Array<{
      id: number;
      name: string;
      path: string;
      sessionCount: number;
      createdAt: string;
      updatedAt: string;
    }>;
    total: number;
  }> {
    const projects = this.projectRepository.findAll();

    const results = projects.map((project) => {
      const sessions = this.sessionRepository.findSessionsByProjectId(project.id!);

      return {
        id: project.id!,
        name: project.name,
        path: project.path,
        sessionCount: sessions.length,
        createdAt: project.createdAt?.toISOString() || new Date().toISOString(),
        updatedAt: project.updatedAt?.toISOString() || new Date().toISOString(),
      };
    });

    return {
      projects: results,
      total: results.length,
    };
  }

  /**
   * 根据 cwd 查找项目
   * 使用前缀匹配（cwd 是 path 的前缀或完全相等）
   */
  private findProjectByCwd(cwd: string): { id: number; name: string } | null {
    const projects = this.projectRepository.findAll();

    // 优先精确匹配
    const exactMatch = projects.find((p) => p.path === cwd);
    if (exactMatch && exactMatch.id) {
      return { id: exactMatch.id, name: exactMatch.name };
    }

    // 其次查找 cwd 是项目路径前缀的情况
    const prefixMatch = projects.find((p) => cwd.startsWith(p.path));
    if (prefixMatch && prefixMatch.id) {
      return { id: prefixMatch.id, name: prefixMatch.name };
    }

    return null;
  }

  /**
   * 获取消息在会话中的序号
   */
  private async getMessageIndex(sessionId: string, messageUuid: string): Promise<number | undefined> {
    const messages = this.sessionRepository.findMessagesBySessionId(sessionId);
    const index = messages.findIndex((msg) => msg.uuid === messageUuid);
    return index !== -1 ? index : undefined;
  }
}
