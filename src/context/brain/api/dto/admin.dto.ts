import { IsString, IsOptional } from 'class-validator';

/**
 * 采集请求 DTO
 */
export class CollectRequestDto {
  /** 项目路径（可选，不指定则采集所有） */
  @IsOptional()
  @IsString()
  projectPath?: string;

  /** 会话 ID（可选，不指定则采集项目下所有会话） */
  @IsOptional()
  @IsString()
  sessionId?: string;
}

/**
 * 采集响应 DTO
 */
export class CollectResponseDto {
  /** 处理的项目数量 */
  projectsProcessed!: number;

  /** 新增的项目数量 */
  projectsCreated!: number;

  /** 处理的会话数量 */
  sessionsProcessed!: number;

  /** 新增/更新的会话数量 */
  sessionsUpdated!: number;

  /** 跳过的会话数量 */
  sessionsSkipped!: number;

  /** 新增的消息数量 */
  messagesCreated!: number;

  /** 处理耗时（毫秒） */
  duration!: number;
}

/**
 * 备份响应 DTO
 */
export class BackupResponseDto {
  /** 备份的项目数量 */
  projectsBackedUp!: number;

  /** 备份的会话数量 */
  sessionsBackedUp!: number;

  /** 跳过的会话数量 */
  sessionsSkipped!: number;

  /** 清理的旧备份目录数量 */
  oldBackupsDeleted!: number;

  /** 处理耗时（毫秒） */
  duration!: number;
}

/**
 * 统计信息响应 DTO
 */
export class StatsResponseDto {
  /** 项目总数 */
  projectCount!: number;

  /** 会话总数 */
  sessionCount!: number;

  /** 消息总数 */
  messageCount!: number;

  /** 数据库大小（字节） */
  databaseSize?: number;

  /** 最后采集时间 */
  lastCollectAt?: string;
}
