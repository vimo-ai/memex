import { Injectable } from '@nestjs/common';
import { ConfigService as NestConfigService } from '@nestjs/config';
import { homedir } from 'os';
import { join } from 'path';
import { MemexConfig } from './memex-config.interface';

/**
 * Memex 配置服务
 * 统一管理所有环境变量和配置项
 */
@Injectable()
export class MemexConfigService implements MemexConfig {
  constructor(private readonly nestConfigService: NestConfigService) {}

  /**
   * HTTP 服务端口
   */
  get port(): number {
    return this.nestConfigService.get<number>('PORT', 10013);
  }

  /**
   * 主数据目录
   */
  get dataDir(): string {
    const defaultPath = join(homedir(), 'memex-data');
    return this.nestConfigService.get<string>('MEMEX_DATA_DIR', defaultPath);
  }

  /**
   * 备份目录
   */
  get backupDir(): string {
    const defaultPath = join(this.dataDir, 'backups');
    return this.nestConfigService.get<string>('MEMEX_BACKUP_DIR', defaultPath);
  }

  /**
   * Claude Code 项目目录路径
   */
  get claudeProjectsPath(): string {
    const defaultPath = join(homedir(), '.claude', 'projects');
    return this.nestConfigService.get<string>(
      'CLAUDE_PROJECTS_PATH',
      defaultPath,
    );
  }

  /**
   * Ollama API 地址
   */
  get ollamaApi(): string {
    return this.nestConfigService.get<string>(
      'OLLAMA_API',
      'http://localhost:11434/api',
    );
  }

  /**
   * 嵌入模型名称
   */
  get embeddingModel(): string {
    return this.nestConfigService.get<string>(
      'EMBEDDING_MODEL',
      'nomic-embed-text',
    );
  }

  /**
   * 获取 SQLite 数据库文件路径
   */
  get dbPath(): string {
    return join(this.dataDir, 'memex.db');
  }

  /**
   * 获取 LanceDB 向量数据库路径
   */
  get lanceDbPath(): string {
    return join(this.dataDir, 'lancedb');
  }
}
