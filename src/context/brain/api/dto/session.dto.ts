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

  /** 数据来源 */
  source?: string;

  /** 渠道/子来源 */
  channel?: string;

  /** 工作目录 */
  cwd?: string;

  /** 模型 */
  model?: string;

  /** 额外元信息 */
  meta?: Record<string, any>;

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
 * 会话搜索响应 DTO
 */
export class SessionSearchResponseDto {
  /** 搜索关键词 */
  query!: string;

  /** 匹配总数 */
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

  /** 数据来源 */
  source?: string;

  /** 渠道/子来源 */
  channel?: string;

  /** 模型 */
  model?: string;

  /** 工具调用 ID */
  toolCallId?: string;

  /** 工具名称 */
  toolName?: string;

  /** 工具参数 */
  toolArgs?: string;

  /** 原始内容 */
  raw?: string;

  /** 额外元信息 */
  meta?: Record<string, any>;

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
