/**
 * 消息类型枚举
 */
export enum MessageType {
  /** 用户消息 */
  USER = 'user',
  /** 助手消息 */
  ASSISTANT = 'assistant',
  /** 工具/函数执行结果 */
  TOOL = 'tool',
}

/**
 * 消息实体
 *
 * 代表一条会话消息
 */
export class MessageEntity {
  /** 数据库自增 ID */
  id?: number;

  /** 消息 UUID */
  uuid: string;

  /** 关联的会话 ID */
  sessionId: string;

  /** 消息类型 */
  type: MessageType;

  /** 消息内容 */
  content: string;

  /** 数据来源（claude/codex/其他） */
  source?: string;

  /** 渠道/子来源（可选，例如 cli/gui） */
  channel?: string;

  /** 模型名称 */
  model?: string;

  /** 关联的工具调用 ID */
  toolCallId?: string;

  /** 工具名称 */
  toolName?: string;

  /** 工具参数（字符串化） */
  toolArgs?: string;

  /** 原始内容（如 reasoning/token 等） */
  raw?: string;

  /** 额外元信息（序列化 JSON） */
  meta?: Record<string, any>;

  /** 消息时间戳 */
  timestamp?: Date;

  /** 创建时间 */
  createdAt?: Date;

  constructor(props: {
    id?: number;
    uuid: string;
    sessionId: string;
    type: MessageType;
    content: string;
    source?: string;
    channel?: string;
    model?: string;
    toolCallId?: string;
    toolName?: string;
    toolArgs?: string;
    raw?: string;
    meta?: Record<string, any>;
    timestamp?: Date;
    createdAt?: Date;
  }) {
    this.id = props.id;
    this.uuid = props.uuid;
    this.sessionId = props.sessionId;
    this.type = props.type;
    this.content = props.content;
    this.source = props.source;
    this.channel = props.channel;
    this.model = props.model;
    this.toolCallId = props.toolCallId;
    this.toolName = props.toolName;
    this.toolArgs = props.toolArgs;
    this.raw = props.raw;
    this.meta = props.meta;
    this.timestamp = props.timestamp;
    this.createdAt = props.createdAt;
  }

  /**
   * 判断是否为用户消息
   */
  isUserMessage(): boolean {
    return this.type === MessageType.USER;
  }

  /**
   * 判断是否为助手消息
   */
  isAssistantMessage(): boolean {
    return this.type === MessageType.ASSISTANT;
  }
}
