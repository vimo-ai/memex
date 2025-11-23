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
}
