import { MessageEntity } from '../../../domain/entities/message.entity';

/**
 * 标准化的会话元数据
 */
export interface AdapterSessionMeta {
  /** 会话 ID */
  id: string;
  /** 数据来源（claude/codex/...） */
  source: string;
  /** 渠道/子来源（可选，例如 cli/gui） */
  channel?: string;
  /** 关联项目真实路径（用于项目去重） */
  projectPath: string;
  /** 项目名称（可选） */
  projectName?: string;
  /** Claude 场景的编码目录名（可选） */
  encodedDirName?: string;
  /** 会话文件完整路径（用于增量检测） */
  sessionPath?: string;
  /** 文件修改时间戳 */
  fileMtime?: number;
  /** 文件大小 */
  fileSize?: number;
  /** 工作目录 */
  cwd?: string;
  /** 默认模型 */
  model?: string;
  /** 额外元信息 */
  meta?: Record<string, any>;
  /** 创建时间（可选） */
  createdAt?: Date;
  /** 更新时间（可选） */
  updatedAt?: Date;
}

/**
 * 适配器解析结果
 */
export interface AdapterParseResult {
  messages: MessageEntity[];
  createdAt?: Date;
  updatedAt?: Date;
  cwd?: string;
  model?: string;
  meta?: Record<string, any>;
}

/**
 * 会话适配器接口
 */
export interface ConversationAdapter {
  /** 数据来源标识 */
  readonly source: string;

  /**
   * 列出当前来源下的所有会话元数据
   */
  listSessions(): Promise<AdapterSessionMeta[]>;

  /**
   * 解析单个会话，返回标准化消息
   */
  parseSession(meta: AdapterSessionMeta): Promise<AdapterParseResult | null>;
}
