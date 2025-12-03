import { Inject, Injectable, Logger, OnModuleInit } from '@nestjs/common';
import { Cron } from '@nestjs/schedule';
import * as fs from 'fs/promises';
import {
  IProjectRepository,
  PROJECT_REPOSITORY,
} from '../../domain/repositories/project.repository.interface';
import {
  ISessionRepository,
  SESSION_REPOSITORY,
} from '../../domain/repositories/session.repository.interface';
import { ProjectEntity } from '../../domain/entities/project.entity';
import { SessionEntity, SessionStatus } from '../../domain/entities/session.entity';
import { AdapterRegistryService } from './adapters/adapter-registry.service';
import { AdapterSessionMeta, ConversationAdapter } from './adapters/conversation-adapter.interface';

/**
 * 采集统计结果
 */
export interface CollectStats {
  /** 处理的项目数量 */
  projectsProcessed: number;
  /** 新增的项目数量 */
  projectsCreated: number;
  /** 处理的会话数量 */
  sessionsProcessed: number;
  /** 新增/更新的会话数量 */
  sessionsUpdated: number;
  /** 跳过的会话数量（无变化） */
  sessionsSkipped: number;
  /** 新增的消息数量 */
  messagesCreated: number;
  /** 处理耗时（毫秒） */
  duration: number;
}

/**
 * 数据采集服务
 *
 * 负责扫描 Claude Code 会话目录，解析并存储到数据库
 * - 服务启动时自动采集
 * - 每日凌晨 2:30 定时采集
 */
@Injectable()
export class CollectorService implements OnModuleInit {
  private readonly logger = new Logger(CollectorService.name);

  constructor(
    @Inject(PROJECT_REPOSITORY)
    private readonly projectRepository: IProjectRepository,
    @Inject(SESSION_REPOSITORY)
    private readonly sessionRepository: ISessionRepository,
    private readonly adapterRegistry: AdapterRegistryService,
  ) {}

  /**
   * 服务启动时执行采集
   */
  async onModuleInit(): Promise<void> {
    this.logger.log('服务启动，开始自动采集...');

    try {
      const stats = await this.collectAll();
      this.logger.log(
        `启动采集完成: ${stats.projectsProcessed} 项目, ` +
          `${stats.sessionsUpdated} 会话已更新, ` +
          `${stats.sessionsSkipped} 会话已跳过, ` +
          `${stats.messagesCreated} 消息已保存, ` +
          `耗时 ${stats.duration}ms`,
      );
    } catch (error) {
      this.logger.error('启动采集失败，但不影响服务启动', error);
    }
  }

  /**
   * 每日定时任务：凌晨 2:30 执行数据采集
   */
  @Cron('30 2 * * *')
  async handleDailyCollectCron(): Promise<void> {
    this.logger.log('开始执行每日定时采集任务...');

    try {
      const stats = await this.collectAll();
      this.logger.log(
        `每日采集完成: ${stats.projectsProcessed} 项目, ` +
          `${stats.sessionsUpdated} 会话已更新, ` +
          `${stats.sessionsSkipped} 会话已跳过, ` +
          `${stats.messagesCreated} 消息已保存, ` +
          `耗时 ${stats.duration}ms`,
      );
    } catch (error) {
      this.logger.error('每日采集任务失败', error);
    }
  }

  /**
   * 全量采集所有项目和会话
   *
   * 扫描所有项目，对比文件变化，增量更新数据库
   */
  async collectAll(): Promise<CollectStats> {
    const startTime = Date.now();
    const stats: CollectStats = {
      projectsProcessed: 0,
      projectsCreated: 0,
      sessionsProcessed: 0,
      sessionsUpdated: 0,
      sessionsSkipped: 0,
      messagesCreated: 0,
      duration: 0,
    };

    const adapters = this.adapterRegistry.getAdapters();
    this.logger.log(`开始全量采集，适配器数量: ${adapters.length}`);

    for (const adapter of adapters) {
      const adapterStats = await this.collectAdapter(adapter);
      stats.projectsProcessed += adapterStats.projectsProcessed;
      stats.projectsCreated += adapterStats.projectsCreated;
      stats.sessionsProcessed += adapterStats.sessionsProcessed;
      stats.sessionsUpdated += adapterStats.sessionsUpdated;
      stats.sessionsSkipped += adapterStats.sessionsSkipped;
      stats.messagesCreated += adapterStats.messagesCreated;
    }

    stats.duration = Date.now() - startTime;
    this.logger.log(
      `全量采集完成: ${stats.projectsProcessed} 项目, ` +
        `${stats.sessionsUpdated} 会话已更新, ` +
        `${stats.sessionsSkipped} 会话已跳过, ` +
        `${stats.messagesCreated} 消息已保存, ` +
        `耗时 ${stats.duration}ms`,
    );

    return stats;
  }

  /**
   * 采集单个项目（按 projectPath 过滤）
   */
  async collectProject(projectPath: string): Promise<CollectStats> {
    const startTime = Date.now();
    const stats = await this.collectAllWithFilter((meta) => meta.projectPath === projectPath);
    stats.duration = Date.now() - startTime;
    return stats;
  }

  /**
   * 同步单个会话
   *
   * @param sessionId 会话 ID (UUID)
   */
  async syncSession(sessionId: string): Promise<CollectStats> {
    const startTime = Date.now();
    const stats: CollectStats = {
      projectsProcessed: 0,
      projectsCreated: 0,
      sessionsProcessed: 1,
      sessionsUpdated: 0,
      sessionsSkipped: 0,
      messagesCreated: 0,
      duration: 0,
    };

    this.logger.log(`同步会话: ${sessionId}`);

    const adapters = this.adapterRegistry.getAdapters();
    for (const adapter of adapters) {
      const metas = await adapter.listSessions();
      const targetMeta = metas.find((m) => m.id === sessionId);
      if (!targetMeta) continue;
      const sessionResult = await this.processAdapterSession(adapter, targetMeta);
      stats.projectsProcessed += sessionResult.projectsProcessed;
      stats.projectsCreated += sessionResult.projectsCreated;
      stats.sessionsUpdated += sessionResult.sessionsUpdated;
      stats.sessionsSkipped += sessionResult.sessionsSkipped;
      stats.messagesCreated += sessionResult.messagesCreated;
      break;
    }

    stats.duration = Date.now() - startTime;
    return stats;
  }

  /**
   * 针对指定适配器执行采集
   */
  private async collectAdapter(adapter: ConversationAdapter): Promise<Omit<CollectStats, 'duration'>> {
    const stats = {
      projectsProcessed: 0,
      projectsCreated: 0,
      sessionsProcessed: 0,
      sessionsUpdated: 0,
      sessionsSkipped: 0,
      messagesCreated: 0,
    };

    const metas = await adapter.listSessions();
    this.logger.log(`[${adapter.source}] 发现 ${metas.length} 个会话`);

    const processedProjects = new Set<string>();

    for (const meta of metas) {
      const result = await this.processAdapterSession(adapter, meta);
      stats.sessionsProcessed++;
      stats.sessionsUpdated += result.sessionsUpdated;
      stats.sessionsSkipped += result.sessionsSkipped;
      stats.messagesCreated += result.messagesCreated;

      if (!processedProjects.has(meta.projectPath)) {
        stats.projectsProcessed++;
        if (result.projectsCreated > 0) {
          stats.projectsCreated += result.projectsCreated;
        }
        processedProjects.add(meta.projectPath);
      }
    }

    return stats;
  }

  /**
   * 支持过滤的全量采集
   */
  private async collectAllWithFilter(
    predicate: (meta: AdapterSessionMeta) => boolean,
  ): Promise<CollectStats> {
    const startTime = Date.now();
    const stats: CollectStats = {
      projectsProcessed: 0,
      projectsCreated: 0,
      sessionsProcessed: 0,
      sessionsUpdated: 0,
      sessionsSkipped: 0,
      messagesCreated: 0,
      duration: 0,
    };

    const adapters = this.adapterRegistry.getAdapters();
    const processedProjects = new Set<string>();
    for (const adapter of adapters) {
      const metas = (await adapter.listSessions()).filter(predicate);
      this.logger.log(`[${adapter.source}] 过滤后 ${metas.length} 个会话`);
      for (const meta of metas) {
        const result = await this.processAdapterSession(adapter, meta);
        stats.sessionsProcessed++;
        stats.sessionsUpdated += result.sessionsUpdated;
        stats.sessionsSkipped += result.sessionsSkipped;
        stats.messagesCreated += result.messagesCreated;
        if (!processedProjects.has(meta.projectPath)) {
          stats.projectsProcessed++;
          processedProjects.add(meta.projectPath);
        }
        stats.projectsCreated += result.projectsCreated;
      }
    }

    stats.duration = Date.now() - startTime;
    return stats;
  }

  /**
   * 处理单个适配器会话
   */
  private async processAdapterSession(
    adapter: ConversationAdapter,
    meta: AdapterSessionMeta,
  ): Promise<Omit<CollectStats, 'sessionsProcessed' | 'duration'>> {
    const result = {
      projectsProcessed: 0,
      projectsCreated: 0,
      sessionsUpdated: 0,
      sessionsSkipped: 0,
      messagesCreated: 0,
    };

    // 1. 准备文件元信息
    let fileMtime = meta.fileMtime;
    let fileSize = meta.fileSize;
    if ((!fileMtime || !fileSize) && meta.sessionPath) {
      try {
        const stat = await fs.stat(meta.sessionPath);
        fileMtime = fileMtime ?? stat.mtimeMs;
        fileSize = fileSize ?? stat.size;
      } catch (error) {
        this.logger.warn(`无法获取会话文件状态: ${meta.sessionPath}`, error as Error);
      }
    }

    // 2. 准备项目
    const projectPath = meta.projectPath || `${adapter.source}-default`;
    const projectName = meta.projectName || projectPath.split('/').filter(Boolean).pop() || projectPath;
    const existingProject = this.projectRepository.findByPath(projectPath);
    const savedProject = this.projectRepository.save(
      ProjectEntity.fromPath(projectPath, meta.encodedDirName, adapter.source),
    );
    result.projectsProcessed = 1;
    result.projectsCreated = existingProject ? 0 : 1;
    if (!existingProject) {
      this.logger.debug(`[${adapter.source}] 新增项目: ${projectName}`);
    }

    // 3. 增量检测
    const existingSession = this.sessionRepository.findSessionById(meta.id);
    if (
      existingSession &&
      fileMtime !== undefined &&
      fileSize !== undefined &&
      !existingSession.hasFileChanged(fileMtime, fileSize)
    ) {
      result.sessionsSkipped = 1;
      return result;
    }

    // 4. 解析会话
    const parseResult = await adapter.parseSession(meta);
    if (!parseResult) {
      this.logger.warn(`[${adapter.source}] 解析会话失败: ${meta.id}`);
      return result;
    }

    // 5. 构造 SessionEntity
    const sessionCreatedAt =
      parseResult.createdAt ??
      meta.createdAt ??
      (fileMtime ? new Date(fileMtime) : undefined);
    const sessionUpdatedAt =
      parseResult.updatedAt ??
      meta.updatedAt ??
      (fileMtime ? new Date(fileMtime) : undefined);

    const sessionEntity = new SessionEntity({
      id: meta.id,
      projectId: savedProject.id!,
      source: adapter.source,
      channel: meta.channel,
      cwd: parseResult.cwd ?? meta.cwd,
      model: parseResult.model ?? meta.model,
      meta: {
        ...(meta.meta || {}),
        ...(parseResult.meta || {}),
      },
      status: SessionStatus.ACTIVE,
      messageCount: parseResult.messages.length,
      fileMtime,
      fileSize,
      createdAt: sessionCreatedAt,
      updatedAt: sessionUpdatedAt,
    });

    this.sessionRepository.saveSession(sessionEntity);

    // 6. 保存消息
    if (parseResult.messages.length > 0) {
      const withSource = parseResult.messages.map((msg) => {
        msg.source = msg.source ?? adapter.source;
        msg.channel = msg.channel ?? meta.channel;
        msg.sessionId = meta.id;
        return msg;
      });
      result.messagesCreated = this.sessionRepository.saveMessages(withSource);
    }

    result.sessionsUpdated = 1;

    this.logger.debug(
      `[${adapter.source}] 会话 ${meta.id} 已更新: ${result.messagesCreated} 条消息`,
    );

    return result;
  }
}
