/**
 * 会话状态枚举
 */
export enum SessionStatus {
  /** 活跃状态 */
  ACTIVE = 'active',
  /** 已关闭 */
  CLOSED = 'closed',
  /** 已归档 */
  ARCHIVED = 'archived',
}

/**
 * 会话实体
 *
 * 代表一个 Claude Code 会话
 */
export class SessionEntity {
  /** 会话 UUID（主键） */
  id: string;

  /** 关联的项目 ID */
  projectId: number;

  /** 会话状态 */
  status: SessionStatus;

  /** 消息数量 */
  messageCount: number;

  /** 文件修改时间戳（用于增量检测） */
  fileMtime?: number;

  /** 文件大小（用于增量检测） */
  fileSize?: number;

  /** 创建时间 */
  createdAt?: Date;

  /** 更新时间 */
  updatedAt?: Date;

  constructor(props: {
    id: string;
    projectId: number;
    status?: SessionStatus;
    messageCount?: number;
    fileMtime?: number;
    fileSize?: number;
    createdAt?: Date;
    updatedAt?: Date;
  }) {
    this.id = props.id;
    this.projectId = props.projectId;
    this.status = props.status ?? SessionStatus.ACTIVE;
    this.messageCount = props.messageCount ?? 0;
    this.fileMtime = props.fileMtime;
    this.fileSize = props.fileSize;
    this.createdAt = props.createdAt;
    this.updatedAt = props.updatedAt;
  }

  /**
   * 更新文件元信息
   */
  updateFileMeta(mtime: number, size: number): void {
    this.fileMtime = mtime;
    this.fileSize = size;
  }

  /**
   * 检查文件是否有变更
   */
  hasFileChanged(mtime: number, size: number): boolean {
    return this.fileMtime !== mtime || this.fileSize !== size;
  }

  /**
   * 增加消息计数
   */
  incrementMessageCount(count: number = 1): void {
    this.messageCount += count;
  }
}
