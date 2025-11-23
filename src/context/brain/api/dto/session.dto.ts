/**
 * 会话响应 DTO
 */
export class SessionResponseDto {
  /** 会话 ID (UUID) */
  id!: string;

  /** 关联的项目 ID */
  projectId!: number;

  /** 会话状态 */
  status!: string;

  /** 消息数量 */
  messageCount!: number;

  /** 创建时间 */
  createdAt!: string;

  /** 更新时间 */
  updatedAt!: string;
}

/**
 * 会话列表响应 DTO
 */
export class SessionListResponseDto {
  /** 会话总数 */
  total!: number;

  /** 会话列表 */
  sessions!: SessionResponseDto[];
}

/**
 * 消息响应 DTO
 */
export class MessageResponseDto {
  /** 消息 ID */
  id!: number;

  /** 消息 UUID */
  uuid!: string;

  /** 会话 ID */
  sessionId!: string;

  /** 消息类型 (user/assistant) */
  type!: string;

  /** 消息内容 */
  content!: string;

  /** 消息时间戳 */
  timestamp?: string;

  /** 创建时间 */
  createdAt!: string;
}

/**
 * 消息列表响应 DTO
 */
export class MessageListResponseDto {
  /** 消息总数 */
  total!: number;

  /** 消息列表 */
  messages!: MessageResponseDto[];
}
