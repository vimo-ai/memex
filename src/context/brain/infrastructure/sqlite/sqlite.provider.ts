import { Provider, Logger, Inject } from '@nestjs/common';
import Database from 'better-sqlite3';
import { mkdirSync, existsSync } from 'fs';
import { dirname } from 'path';
import { ALL_SCHEMA_STATEMENTS } from './sqlite.schema';
import { MemexConfigService } from '../../../../config';

/** SQLite 数据库注入 Token */
export const SQLITE_DB = Symbol('SQLITE_DB');

/** 数据库类型导出 */
export type SqliteDatabase = Database.Database;

/**
 * 创建 SQLite 数据库 Provider
 *
 * 职责：
 * 1. 确保数据目录存在
 * 2. 创建数据库连接
 * 3. 初始化表结构
 * 4. 启用 WAL 模式（提高并发性能）
 */
export const SqliteProvider: Provider = {
  provide: SQLITE_DB,
  inject: [MemexConfigService],
  useFactory: (configService: MemexConfigService): SqliteDatabase => {
    const logger = new Logger('SqliteProvider');

    const dbPath = configService.dbPath;
    const dataDir = dirname(dbPath);

    // 确保数据目录存在
    if (!existsSync(dataDir)) {
      mkdirSync(dataDir, { recursive: true });
      logger.log(`创建数据目录: ${dataDir}`);
    }

    logger.log(`连接数据库: ${dbPath}`);

    // 创建数据库连接
    const db = new Database(dbPath);

    // 启用 WAL 模式，提高并发性能
    db.pragma('journal_mode = WAL');

    // 启用外键约束
    db.pragma('foreign_keys = ON');

    // 初始化表结构
    initializeSchema(db, logger);

    logger.log('数据库初始化完成');

    return db;
  },
};

/**
 * 初始化数据库表结构
 */
function initializeSchema(db: SqliteDatabase, logger: Logger): void {
  // 使用事务执行所有 schema 语句
  const initSchema = db.transaction(() => {
    for (const statement of ALL_SCHEMA_STATEMENTS) {
      db.exec(statement);
    }
  });

  try {
    initSchema();
    logger.log('表结构初始化成功');
  } catch (error) {
    logger.error('表结构初始化失败', error);
    throw error;
  }
}
