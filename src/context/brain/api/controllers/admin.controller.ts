import { Controller, Get, Post, Body, Inject } from '@nestjs/common';
import { CollectorService } from '../../application/services/collector.service';
import { BackupService } from '../../application/services/backup.service';
import {
  IProjectRepository,
  PROJECT_REPOSITORY,
} from '../../domain/repositories/project.repository.interface';
import {
  ISessionRepository,
  SESSION_REPOSITORY,
} from '../../domain/repositories/session.repository.interface';
import {
  CollectRequestDto,
  CollectResponseDto,
  BackupResponseDto,
  StatsResponseDto,
} from '../dto/admin.dto';

/**
 * 管理控制器
 *
 * 提供系统管理相关的 HTTP API
 */
@Controller('admin')
export class AdminController {
  constructor(
    private readonly collectorService: CollectorService,
    private readonly backupService: BackupService,
    @Inject(PROJECT_REPOSITORY)
    private readonly projectRepository: IProjectRepository,
    @Inject(SESSION_REPOSITORY)
    private readonly sessionRepository: ISessionRepository,
  ) {}

  /**
   * 触发数据采集
   *
   * POST /api/admin/collect
   *
   * 请求体可选参数：
   * - projectPath: 指定项目路径，只采集该项目
   * - sessionId: 指定会话 ID，只同步该会话
   */
  @Post('collect')
  async collect(@Body() body: CollectRequestDto): Promise<CollectResponseDto> {
    let stats;

    if (body.sessionId) {
      // 同步单个会话
      stats = await this.collectorService.syncSession(body.sessionId);
    } else if (body.projectPath) {
      // 采集单个项目
      stats = await this.collectorService.collectProject(body.projectPath);
    } else {
      // 全量采集
      stats = await this.collectorService.collectAll();
    }

    return {
      projectsProcessed: stats.projectsProcessed,
      projectsCreated: stats.projectsCreated,
      sessionsProcessed: stats.sessionsProcessed,
      sessionsUpdated: stats.sessionsUpdated,
      sessionsSkipped: stats.sessionsSkipped,
      messagesCreated: stats.messagesCreated,
      duration: stats.duration,
    };
  }

  /**
   * 触发备份
   *
   * POST /api/admin/backup
   */
  @Post('backup')
  async backup(): Promise<BackupResponseDto> {
    const stats = await this.backupService.runDailyBackup();

    return {
      projectsBackedUp: stats.projectsBackedUp,
      sessionsBackedUp: stats.sessionsBackedUp,
      sessionsSkipped: stats.sessionsSkipped,
      oldBackupsDeleted: stats.oldBackupsDeleted,
      duration: stats.duration,
    };
  }

  /**
   * 获取统计信息
   *
   * GET /api/admin/stats
   */
  @Get('stats')
  getStats(): StatsResponseDto {
    return {
      projectCount: this.projectRepository.count(),
      sessionCount: this.sessionRepository.countSessions(),
      messageCount: this.sessionRepository.countMessages(),
    };
  }

  /**
   * 修复缺失的 cwd 和 model
   *
   * POST /api/admin/fix-metadata
   *
   * 将 cwd 为空的会话标记为需要重新采集，然后自动触发全量采集
   */
  @Post('fix-metadata')
  async fixMetadata(): Promise<{
    sessionsReset: number;
    collectStats: CollectResponseDto;
  }> {
    // 1. 重置 cwd 为空的会话的 file_mtime
    const sessionsReset = this.sessionRepository.resetFileMtime(
      "cwd IS NULL OR cwd = ''",
    );

    // 2. 触发全量采集
    const stats = await this.collectorService.collectAll();

    return {
      sessionsReset,
      collectStats: {
        projectsProcessed: stats.projectsProcessed,
        projectsCreated: stats.projectsCreated,
        sessionsProcessed: stats.sessionsProcessed,
        sessionsUpdated: stats.sessionsUpdated,
        sessionsSkipped: stats.sessionsSkipped,
        messagesCreated: stats.messagesCreated,
        duration: stats.duration,
      },
    };
  }

  /**
   * 合并重复的项目
   *
   * POST /api/admin/merge-projects
   *
   * 自动检测相同路径的项目并合并（保留 claude 来源的项目，合并其他来源的会话）
   */
  @Post('merge-projects')
  mergeDuplicateProjects(): {
    mergedCount: number;
    details: Array<{ from: number; to: number; sessionsMoved: number }>;
  } {
    const allProjects = this.projectRepository.findAll();
    const pathMap = new Map<string, { claude?: number; others: number[] }>();

    // 按路径分组项目
    for (const project of allProjects) {
      if (!pathMap.has(project.path)) {
        pathMap.set(project.path, { others: [] });
      }
      const group = pathMap.get(project.path)!;
      if (project.source === 'claude') {
        group.claude = project.id!;
      } else {
        group.others.push(project.id!);
      }
    }

    const details: Array<{ from: number; to: number; sessionsMoved: number }> =
      [];

    // 合并相同路径的项目
    for (const [path, group] of pathMap) {
      // 如果只有一个项目，跳过
      if (!group.claude && group.others.length <= 1) continue;
      if (group.claude && group.others.length === 0) continue;

      // 确定目标项目（优先 claude，否则用第一个）
      const targetId = group.claude ?? group.others[0];
      const sourceIds = group.claude
        ? group.others
        : group.others.slice(1);

      // 合并会话
      for (const sourceId of sourceIds) {
        const sessionsMoved = this.sessionRepository.updateProjectId(
          sourceId,
          targetId,
        );
        if (sessionsMoved > 0) {
          details.push({ from: sourceId, to: targetId, sessionsMoved });
        }
        // 删除源项目
        this.projectRepository.delete(sourceId);
      }
    }

    return {
      mergedCount: details.length,
      details,
    };
  }
}
