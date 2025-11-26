import { Injectable, Logger, OnModuleInit } from '@nestjs/common';
import { Cron } from '@nestjs/schedule';
import * as fs from 'fs/promises';
import * as path from 'path';
import { scanProjects, scanSessions, buildSessionPath } from '@vimo-ai/vlaude-shared-core';
import { MemexConfigService } from '../../../../config';

/**
 * 备份统计结果
 */
export interface BackupStats {
  /** 备份的项目数量 */
  projectsBackedUp: number;
  /** 备份的会话数量 */
  sessionsBackedUp: number;
  /** 跳过的会话数量（已是最新） */
  sessionsSkipped: number;
  /** 清理的旧备份目录数量 */
  oldBackupsDeleted: number;
  /** 处理耗时（毫秒） */
  duration: number;
}

/**
 * 每日增量备份服务
 *
 * 职责:
 * - 每日自动备份 Claude Code 会话文件
 * - 增量备份：只复制有更新的文件
 * - 自动清理超过 30 天的旧备份
 */
@Injectable()
export class BackupService implements OnModuleInit {
  private readonly logger = new Logger(BackupService.name);

  /** Claude projects 源目录 */
  private readonly claudeProjectsPath: string;
  /** 备份根目录 */
  private readonly backupBasePath: string;
  /** 备份保留天数 */
  private readonly retentionDays = 30;

  constructor(private readonly configService: MemexConfigService) {
    this.claudeProjectsPath = this.configService.claudeProjectsPath;
    this.backupBasePath = this.configService.backupDir;
  }

  /**
   * 服务启动时执行备份
   */
  async onModuleInit(): Promise<void> {
    this.logger.log('服务启动，开始执行初始备份...');

    try {
      const stats = await this.runDailyBackup();
      this.logger.log(
        `启动备份完成: ${stats.projectsBackedUp} 项目, ` +
          `${stats.sessionsBackedUp} 会话已备份, ` +
          `${stats.sessionsSkipped} 会话已跳过, ` +
          `耗时 ${stats.duration}ms`,
      );
    } catch (error) {
      this.logger.error('启动备份失败', error);
    }
  }

  /**
   * 每日定时任务：凌晨 2:00 执行备份
   */
  @Cron('0 2 * * *')
  async handleDailyBackupCron(): Promise<void> {
    this.logger.log('开始执行每日定时备份任务...');

    try {
      const stats = await this.runDailyBackup();
      this.logger.log(
        `每日备份完成: ${stats.projectsBackedUp} 项目, ` +
          `${stats.sessionsBackedUp} 会话已备份, ` +
          `${stats.sessionsSkipped} 会话已跳过, ` +
          `${stats.oldBackupsDeleted} 旧备份已清理, ` +
          `耗时 ${stats.duration}ms`,
      );
    } catch (error) {
      this.logger.error('每日备份任务失败', error);
    }
  }

  /**
   * 执行每日备份
   *
   * 1. 扫描所有项目和会话
   * 2. 对每个会话执行增量备份
   * 3. 清理超过 30 天的旧备份
   */
  async runDailyBackup(): Promise<BackupStats> {
    const startTime = Date.now();
    const stats: BackupStats = {
      projectsBackedUp: 0,
      sessionsBackedUp: 0,
      sessionsSkipped: 0,
      oldBackupsDeleted: 0,
      duration: 0,
    };

    // 获取今日日期作为备份目录名
    const today = this.formatDate(new Date());
    this.logger.log(`开始每日备份，日期: ${today}`);

    // 1. 扫描所有项目
    const projects = await scanProjects(this.claudeProjectsPath);
    this.logger.log(`发现 ${projects.length} 个项目`);

    // 2. 对每个项目的会话进行备份
    for (const project of projects) {
      const sessions = await scanSessions(
        this.claudeProjectsPath,
        project.encodedDirName,
        project.path,
      );

      let projectHasBackup = false;

      for (const session of sessions) {
        const result = await this.backupSession(
          project.path,
          project.encodedDirName,
          session.id,
        );

        if (result.backedUp) {
          stats.sessionsBackedUp++;
          projectHasBackup = true;
        } else if (result.skipped) {
          stats.sessionsSkipped++;
        }
      }

      if (projectHasBackup || sessions.length > 0) {
        stats.projectsBackedUp++;
      }
    }

    // 3. 清理旧备份
    stats.oldBackupsDeleted = await this.cleanupOldBackups();

    stats.duration = Date.now() - startTime;
    return stats;
  }

  /**
   * 备份单个会话文件
   *
   * 增量逻辑：
   * - 如果备份文件不存在，直接复制
   * - 如果源文件 mtime > 备份文件 mtime，则覆盖复制
   * - 否则跳过
   *
   * @param projectPath 项目真实路径（用于日志）
   * @param encodedDirName 编码后的目录名
   * @param sessionId 会话 ID
   */
  async backupSession(
    projectPath: string,
    encodedDirName: string,
    sessionId: string,
  ): Promise<{ backedUp: boolean; skipped: boolean }> {
    const result = { backedUp: false, skipped: false };

    try {
      // 构建源文件路径
      const sourcePath = buildSessionPath(
        this.claudeProjectsPath,
        encodedDirName,
        sessionId,
      );

      // 获取源文件信息
      const sourceStat = await fs.stat(sourcePath);

      // 构建备份目标路径
      const today = this.formatDate(new Date());
      const backupDir = path.join(this.backupBasePath, today, encodedDirName);
      const backupPath = path.join(backupDir, `${sessionId}.jsonl`);

      // 检查备份文件是否存在及其修改时间
      let needsBackup = true;
      try {
        const backupStat = await fs.stat(backupPath);
        // 如果源文件修改时间不比备份文件新，跳过
        if (sourceStat.mtimeMs <= backupStat.mtimeMs) {
          needsBackup = false;
          result.skipped = true;
        }
      } catch {
        // 备份文件不存在，需要备份
      }

      if (needsBackup) {
        // 确保备份目录存在
        await fs.mkdir(backupDir, { recursive: true });

        // 复制文件
        await fs.copyFile(sourcePath, backupPath);

        // 保持原始文件的修改时间
        await fs.utimes(backupPath, sourceStat.atime, sourceStat.mtime);

        this.logger.verbose(`已备份会话: ${sessionId} (${projectPath})`);
        result.backedUp = true;
      }
    } catch (error) {
      this.logger.warn(`备份会话 ${sessionId} 失败`, error);
    }

    return result;
  }

  /**
   * 清理超过保留期的旧备份
   *
   * 遍历备份根目录下的日期目录，删除超过 30 天的目录
   */
  async cleanupOldBackups(): Promise<number> {
    let deletedCount = 0;

    try {
      // 确保备份目录存在
      await fs.mkdir(this.backupBasePath, { recursive: true });

      // 列出所有日期目录
      const entries = await fs.readdir(this.backupBasePath, {
        withFileTypes: true,
      });

      const cutoffDate = new Date();
      cutoffDate.setDate(cutoffDate.getDate() - this.retentionDays);

      for (const entry of entries) {
        if (!entry.isDirectory()) {
          continue;
        }

        // 尝试解析日期
        const dirDate = this.parseDate(entry.name);
        if (!dirDate) {
          this.logger.debug(`跳过非日期格式目录: ${entry.name}`);
          continue;
        }

        // 检查是否超过保留期
        if (dirDate < cutoffDate) {
          const dirPath = path.join(this.backupBasePath, entry.name);

          try {
            await fs.rm(dirPath, { recursive: true, force: true });
            this.logger.log(`已清理过期备份: ${entry.name}`);
            deletedCount++;
          } catch (error) {
            this.logger.warn(`清理备份目录失败: ${entry.name}`, error);
          }
        }
      }
    } catch (error) {
      this.logger.error('清理旧备份失败', error);
    }

    return deletedCount;
  }

  /**
   * 格式化日期为 yyyy-MM-dd
   */
  private formatDate(date: Date): string {
    const year = date.getFullYear();
    const month = String(date.getMonth() + 1).padStart(2, '0');
    const day = String(date.getDate()).padStart(2, '0');
    return `${year}-${month}-${day}`;
  }

  /**
   * 解析 yyyy-MM-dd 格式的日期字符串
   */
  private parseDate(dateStr: string): Date | null {
    const match = dateStr.match(/^(\d{4})-(\d{2})-(\d{2})$/);
    if (!match) {
      return null;
    }

    const year = parseInt(match[1], 10);
    const month = parseInt(match[2], 10) - 1;
    const day = parseInt(match[3], 10);

    const date = new Date(year, month, day);

    // 验证日期有效性
    if (
      date.getFullYear() !== year ||
      date.getMonth() !== month ||
      date.getDate() !== day
    ) {
      return null;
    }

    return date;
  }
}
