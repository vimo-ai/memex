import { Module } from '@nestjs/common';
import { ScheduleModule } from '@nestjs/schedule';
import { DiscoveryModule } from '@nestjs/core';
import { SqliteProvider } from './infrastructure/sqlite/sqlite.provider';
import { ProjectSqliteRepository } from './infrastructure/sqlite/project.sqlite.repository';
import { SessionSqliteRepository } from './infrastructure/sqlite/session.sqlite.repository';
import { LanceDbProvider } from './infrastructure/lancedb/lancedb.provider';
import { VectorLanceDbRepository } from './infrastructure/lancedb/vector.lancedb.repository';
import { PROJECT_REPOSITORY } from './domain/repositories/project.repository.interface';
import { SESSION_REPOSITORY } from './domain/repositories/session.repository.interface';
import { ParserService } from './application/services/parser.service';
import { CollectorService } from './application/services/collector.service';
import { BackupService } from './application/services/backup.service';
import { SearchService } from './application/services/search.service';
import { EmbeddingService } from './application/services/embedding.service';
import { ChunkingService } from './application/services/chunking.service';
import { HybridSearchService } from './application/services/hybrid-search.service';
import { VectorIndexerService } from './application/services/vector-indexer.service';
import { RagService } from './application/services/rag.service';
import { MCPToolsService } from './application/services/mcp-tools.service';
import { MCPRegistryService } from './mcp/services/mcp-registry.service';
import { ProjectController } from './api/controllers/project.controller';
import { SessionController } from './api/controllers/session.controller';
import { SearchController } from './api/controllers/search.controller';
import { AdminController } from './api/controllers/admin.controller';
import { SemanticSearchController, EmbeddingController } from './api/controllers/semantic-search.controller';
import { RagController } from './api/controllers/rag.controller';
import { MCPController } from './api/controllers/mcp.controller';

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
  imports: [ScheduleModule.forRoot(), DiscoveryModule],
  controllers: [
    ProjectController,
    SessionController,
    SearchController,
    AdminController,
    SemanticSearchController,
    EmbeddingController,
    RagController,
    MCPController,
  ],
  providers: [
    // SQLite 数据库连接
    SqliteProvider,
    // LanceDB 向量数据库连接
    LanceDbProvider,
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
    // 向量仓储
    VectorLanceDbRepository,
    // 应用服务
    ParserService,
    CollectorService,
    BackupService,
    SearchService,
    EmbeddingService,
    ChunkingService,
    HybridSearchService,
    VectorIndexerService,
    RagService,
    // MCP 相关服务
    MCPRegistryService,
    MCPToolsService,
  ],
  exports: [
    PROJECT_REPOSITORY,
    SESSION_REPOSITORY,
    ParserService,
    CollectorService,
    BackupService,
    SearchService,
    EmbeddingService,
    ChunkingService,
    HybridSearchService,
    VectorIndexerService,
  ],
})
export class BrainContext {}
