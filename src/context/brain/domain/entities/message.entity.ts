/**
 * 消息类型枚举
 */
export enum MessageType {
  /** 用户消息 */
  USER = 'user',
  /** 助手消息 */
  ASSISTANT = 'assistant',
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
    timestamp?: Date;
    createdAt?: Date;
  }) {
    this.id = props.id;
    this.uuid = props.uuid;
    this.sessionId = props.sessionId;
    this.type = props.type;
    this.content = props.content;
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
