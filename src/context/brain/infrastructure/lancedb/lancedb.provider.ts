import { Provider, Logger } from '@nestjs/common';
import * as lancedb from '@lancedb/lancedb';
import { homedir } from 'os';
import { join } from 'path';
import { mkdirSync, existsSync } from 'fs';

/** LanceDB 数据库注入 Token */
export const LANCEDB_CONNECTION = Symbol('LANCEDB_CONNECTION');

/** LanceDB 连接类型 */
export type LanceDbConnection = lancedb.Connection;

/** 默认数据目录 */
const DATA_DIR = join(homedir(), 'memex-data');

/** 向量数据库目录 */
const VECTORS_DIR = join(DATA_DIR, 'vectors');

/**
 * 创建 LanceDB Provider
 *
 * 职责：
 * 1. 确保向量数据目录存在
 * 2. 创建 LanceDB 连接
 * 3. 异步初始化（LanceDB connect 是异步的）
 */
export const LanceDbProvider: Provider = {
  provide: LANCEDB_CONNECTION,
  useFactory: async (): Promise<LanceDbConnection> => {
    const logger = new Logger('LanceDbProvider');

    // 确保数据目录存在
    if (!existsSync(VECTORS_DIR)) {
      mkdirSync(VECTORS_DIR, { recursive: true });
      logger.log(`创建向量数据目录: ${VECTORS_DIR}`);
    }

    logger.log(`连接 LanceDB: ${VECTORS_DIR}`);

    // 创建 LanceDB 连接
    const connection = await lancedb.connect(VECTORS_DIR);

    logger.log('LanceDB 连接成功');

    return connection;
  },
};
