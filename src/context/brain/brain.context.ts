import { Module } from '@nestjs/common';
import { ScheduleModule } from '@nestjs/schedule';
import { SqliteProvider } from './infrastructure/sqlite/sqlite.provider';
import { ProjectSqliteRepository } from './infrastructure/sqlite/project.sqlite.repository';
import { SessionSqliteRepository } from './infrastructure/sqlite/session.sqlite.repository';
import { PROJECT_REPOSITORY } from './domain/repositories/project.repository.interface';
import { SESSION_REPOSITORY } from './domain/repositories/session.repository.interface';
import { ParserService } from './application/services/parser.service';
import { CollectorService } from './application/services/collector.service';
import { BackupService } from './application/services/backup.service';
import { SearchService } from './application/services/search.service';
import { ProjectController } from './api/controllers/project.controller';
import { SessionController } from './api/controllers/session.controller';
import { SearchController } from './api/controllers/search.controller';
import { AdminController } from './api/controllers/admin.controller';

/**
 * Brain 上下文模块
 *
 * 核心职责:
 * - 扫描和解析 Claude Code 会话文件
 * - 存储会话数据到 SQLite
 * - 提供全文搜索能力
 * - 暴露 HTTP API
 *
 * 目录结构:
 * - api/controllers: HTTP 控制器
 * - application/services: 应用服务（用例编排）
 * - domain/entities: 领域实体
 * - domain/repositories: 仓储接口
 * - infrastructure/sqlite: SQLite 实现
 * - infrastructure/watcher: 文件监听
 */
@Module({
  imports: [ScheduleModule.forRoot()],
  controllers: [ProjectController, SessionController, SearchController, AdminController],
  providers: [
    // SQLite 数据库连接
    SqliteProvider,
    // 项目仓储
    {
      provide: PROJECT_REPOSITORY,
      useClass: ProjectSqliteRepository,
    },
    // 会话仓储
    {
      provide: SESSION_REPOSITORY,
      useClass: SessionSqliteRepository,
    },
    // 应用服务
    ParserService,
    CollectorService,
    BackupService,
    SearchService,
  ],
  exports: [PROJECT_REPOSITORY, SESSION_REPOSITORY, ParserService, CollectorService, BackupService, SearchService],
})
export class BrainContext {}
