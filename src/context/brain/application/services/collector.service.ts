import { Inject, Injectable, Logger, OnModuleInit } from '@nestjs/common';
import { Cron } from '@nestjs/schedule';
import * as fs from 'fs/promises';
import {
  scanProjects,
  scanSessions,
  buildSessionPath,
  type ClaudeProjectInfo,
  type ClaudeSessionMeta,
} from '@vlaude/shared-core';
import { MemexConfigService } from '../../../../config';
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
import { ParserService } from './parser.service';

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

  /** Claude projects 目录路径 */
  private readonly claudeProjectsPath: string;

  constructor(
    @Inject(PROJECT_REPOSITORY)
    private readonly projectRepository: IProjectRepository,
    @Inject(SESSION_REPOSITORY)
    private readonly sessionRepository: ISessionRepository,
    private readonly parserService: ParserService,
    private readonly configService: MemexConfigService,
  ) {
    this.claudeProjectsPath = this.configService.claudeProjectsPath;
  }

  /**
   * 服务启动时执行采集
   */
  async onModuleInit(): Promise<void> {
    this.logger.log('服务启动，开始执行初始数据采集...');

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

    this.logger.log(`开始全量采集，扫描目录: ${this.claudeProjectsPath}`);

    // 1. 扫描所有项目
    const projects = await scanProjects(this.claudeProjectsPath);
    this.logger.log(`发现 ${projects.length} 个项目`);

    // 2. 处理每个项目
    for (const projectInfo of projects) {
      const projectStats = await this.collectProjectInternal(projectInfo);
      stats.projectsProcessed++;
      stats.projectsCreated += projectStats.projectsCreated;
      stats.sessionsProcessed += projectStats.sessionsProcessed;
      stats.sessionsUpdated += projectStats.sessionsUpdated;
      stats.sessionsSkipped += projectStats.sessionsSkipped;
      stats.messagesCreated += projectStats.messagesCreated;
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
   * 采集单个项目
   *
   * @param projectPath 项目的真实路径
   */
  async collectProject(projectPath: string): Promise<CollectStats> {
    const startTime = Date.now();
    const stats: CollectStats = {
      projectsProcessed: 1,
      projectsCreated: 0,
      sessionsProcessed: 0,
      sessionsUpdated: 0,
      sessionsSkipped: 0,
      messagesCreated: 0,
      duration: 0,
    };

    this.logger.log(`采集项目: ${projectPath}`);

    // 扫描项目列表并查找匹配的项目
    const projects = await scanProjects(this.claudeProjectsPath);
    const projectInfo = projects.find((p) => p.path === projectPath);

    if (!projectInfo) {
      this.logger.warn(`未找到项目: ${projectPath}`);
      stats.duration = Date.now() - startTime;
      return stats;
    }

    const projectStats = await this.collectProjectInternal(projectInfo);
    Object.assign(stats, {
      ...projectStats,
      projectsProcessed: 1,
      duration: Date.now() - startTime,
    });

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

    // 先从数据库查找会话
    const existingSession = this.sessionRepository.findSessionById(sessionId);

    if (existingSession) {
      // 已存在会话，从关联的项目获取信息
      const project = this.projectRepository.findById(existingSession.projectId);
      if (!project) {
        this.logger.warn(`会话 ${sessionId} 关联的项目不存在`);
        stats.duration = Date.now() - startTime;
        return stats;
      }

      const sessionStats = await this.syncSessionInternal(
        project.encodedDirName,
        sessionId,
        existingSession.projectId,
      );
      Object.assign(stats, sessionStats);
    } else {
      // 未找到会话，需要在所有项目中搜索
      const projects = await scanProjects(this.claudeProjectsPath);

      for (const projectInfo of projects) {
        // 检查会话文件是否存在于该项目
        const sessionPath = buildSessionPath(
          this.claudeProjectsPath,
          projectInfo.encodedDirName,
          sessionId,
        );

        try {
          await fs.access(sessionPath);

          // 找到了，先确保项目存在
          const savedProject = this.projectRepository.save(
            ProjectEntity.fromPath(projectInfo.path, projectInfo.encodedDirName),
          );
          stats.projectsProcessed = 1;

          if (!this.projectRepository.findByPath(projectInfo.path)) {
            stats.projectsCreated = 1;
          }

          const sessionStats = await this.syncSessionInternal(
            projectInfo.encodedDirName,
            sessionId,
            savedProject.id!,
          );
          Object.assign(stats, {
            ...sessionStats,
            projectsProcessed: 1,
            projectsCreated: stats.projectsCreated,
          });
          break;
        } catch {
          // 文件不存在，继续搜索下一个项目
        }
      }
    }

    stats.duration = Date.now() - startTime;
    return stats;
  }

  /**
   * 内部方法：采集单个项目
   */
  private async collectProjectInternal(
    projectInfo: ClaudeProjectInfo,
  ): Promise<Omit<CollectStats, 'duration'>> {
    const stats = {
      projectsProcessed: 1,
      projectsCreated: 0,
      sessionsProcessed: 0,
      sessionsUpdated: 0,
      sessionsSkipped: 0,
      messagesCreated: 0,
    };

    // 1. 保存/更新项目
    const existingProject = this.projectRepository.findByPath(projectInfo.path);
    const savedProject = this.projectRepository.save(
      ProjectEntity.fromPath(projectInfo.path, projectInfo.encodedDirName),
    );

    if (!existingProject) {
      stats.projectsCreated = 1;
      this.logger.debug(`新增项目: ${projectInfo.name}`);
    }

    // 2. 扫描会话
    const sessions = await scanSessions(
      this.claudeProjectsPath,
      projectInfo.encodedDirName,
      projectInfo.path,
    );

    // 3. 处理每个会话
    for (const sessionMeta of sessions) {
      stats.sessionsProcessed++;

      const sessionStats = await this.processSession(
        sessionMeta,
        projectInfo.encodedDirName,
        savedProject.id!,
      );

      stats.sessionsUpdated += sessionStats.updated ? 1 : 0;
      stats.sessionsSkipped += sessionStats.skipped ? 1 : 0;
      stats.messagesCreated += sessionStats.messagesCreated;
    }

    return stats;
  }

  /**
   * 内部方法：处理单个会话
   *
   * 包含增量检测逻辑
   */
  private async processSession(
    sessionMeta: ClaudeSessionMeta,
    encodedDirName: string,
    projectId: number,
  ): Promise<{ updated: boolean; skipped: boolean; messagesCreated: number }> {
    const result = { updated: false, skipped: false, messagesCreated: 0 };

    // 1. 获取文件元信息
    const sessionPath = buildSessionPath(
      this.claudeProjectsPath,
      encodedDirName,
      sessionMeta.id,
    );

    let fileMtime: number;
    let fileSize: number;

    try {
      const fileStat = await fs.stat(sessionPath);
      fileMtime = fileStat.mtimeMs;
      fileSize = fileStat.size;
    } catch (error) {
      this.logger.warn(`无法获取会话文件状态: ${sessionPath}`, error);
      return result;
    }

    // 2. 检查是否需要更新
    const existingSession = this.sessionRepository.findSessionById(sessionMeta.id);

    if (existingSession && !existingSession.hasFileChanged(fileMtime, fileSize)) {
      result.skipped = true;
      return result;
    }

    // 3. 解析消息
    const parseResult = await this.parserService.parseSession(
      this.claudeProjectsPath,
      encodedDirName,
      sessionMeta.id,
    );

    if (!parseResult) {
      this.logger.warn(`解析会话失败: ${sessionMeta.id}`);
      return result;
    }

    // 4. 先保存/更新会话（消息有外键指向会话）
    // 从消息中提取会话时间
    let sessionCreatedAt: Date | undefined;
    let sessionUpdatedAt: Date | undefined;

    if (parseResult.messages.length > 0) {
      // 第一条消息的时间作为会话创建时间
      const firstMsg = parseResult.messages[0];
      if (firstMsg.timestamp) {
        sessionCreatedAt = firstMsg.timestamp;
      }
      // 最后一条消息的时间作为会话更新时间
      const lastMsg = parseResult.messages[parseResult.messages.length - 1];
      if (lastMsg.timestamp) {
        sessionUpdatedAt = lastMsg.timestamp;
      }
    }

    // 如果没有从消息获取到时间，使用文件 mtime
    if (!sessionCreatedAt) {
      sessionCreatedAt = new Date(fileMtime);
    }
    if (!sessionUpdatedAt) {
      sessionUpdatedAt = new Date(fileMtime);
    }

    const sessionEntity = new SessionEntity({
      id: sessionMeta.id,
      projectId,
      status: SessionStatus.ACTIVE,
      messageCount: parseResult.messages.length,
      fileMtime,
      fileSize,
      createdAt: sessionCreatedAt,
      updatedAt: sessionUpdatedAt,
    });

    this.sessionRepository.saveSession(sessionEntity);

    // 5. 保存消息
    if (parseResult.messages.length > 0) {
      result.messagesCreated = this.sessionRepository.saveMessages(parseResult.messages);
    }

    result.updated = true;

    this.logger.debug(
      `会话 ${sessionMeta.id} 已更新: ${result.messagesCreated} 条消息`,
    );

    return result;
  }

  /**
   * 内部方法：同步单个会话
   */
  private async syncSessionInternal(
    encodedDirName: string,
    sessionId: string,
    projectId: number,
  ): Promise<Omit<CollectStats, 'projectsProcessed' | 'projectsCreated' | 'duration'>> {
    const stats = {
      sessionsProcessed: 1,
      sessionsUpdated: 0,
      sessionsSkipped: 0,
      messagesCreated: 0,
    };

    // 构造 sessionMeta（虽然不完整，但 processSession 只需要 id）
    const sessionMeta: ClaudeSessionMeta = {
      id: sessionId,
      projectPath: '', // 不需要
      createdAt: new Date(),
      lastUpdated: new Date(),
      messageCount: 0,
    };

    const result = await this.processSession(sessionMeta, encodedDirName, projectId);
    stats.sessionsUpdated = result.updated ? 1 : 0;
    stats.sessionsSkipped = result.skipped ? 1 : 0;
    stats.messagesCreated = result.messagesCreated;

    return stats;
  }
}
