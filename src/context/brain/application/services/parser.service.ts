import { Injectable, Logger } from '@nestjs/common';
import {
  readSessionMessages,
  buildSessionPath,
  type SessionMessagesResult,
} from '@vlaude/shared-core';
import { MessageEntity, MessageType } from '../../domain/entities/message.entity';

/**
 * JSONL 消息条目类型
 * 对应 Claude Code 的消息格式
 *
 * 参考：/docs/17-user-message-scenarios.md
 */
interface JsonlMessageEntry {
  uuid: string;
  type: 'user' | 'message' | 'summary' | 'system' | 'assistant';
  message?: {
    id?: string;
    role?: 'user' | 'assistant';
    content?: string | ContentBlock[];
  };
  timestamp?: string;

  // User 消息的特殊标记字段
  /** 工具执行结果（占 80% 的 user 消息） */
  toolUseResult?: {
    toolName?: string;
    result?: string;
    isError?: boolean;
  };
  /** 仅在 Transcript 中可见 */
  isVisibleInTranscriptOnly?: boolean;
  /** 压缩摘要 */
  isCompactSummary?: boolean;
  /** 元数据消息 */
  isMeta?: boolean;
  /** 思考元数据 */
  thinkingMetadata?: {
    thinkingBudget?: number;
    thinkingEnabled?: boolean;
  };
}

/**
 * 内容块类型
 */
interface ContentBlock {
  type: string;
  text?: string;
  // tool_result 类型的字段
  tool_use_id?: string;
  content?: string | ContentBlock[];
  is_error?: boolean;
}

/**
 * 解析会话结果
 */
export interface ParseSessionResult {
  messages: MessageEntity[];
  total: number;
}

/**
 * 解析服务
 *
 * 负责解析 Claude Code JSONL 文件并转换为 MessageEntity
 */
@Injectable()
export class ParserService {
  private readonly logger = new Logger(ParserService.name);

  /**
   * 解析会话文件并返回消息实体列表
   *
   * @param claudeProjectsPath Claude projects 目录路径
   * @param encodedDirName 编码的目录名
   * @param sessionId 会话 ID
   * @returns 解析后的消息实体列表
   */
  async parseSession(
    claudeProjectsPath: string,
    encodedDirName: string,
    sessionId: string,
  ): Promise<ParseSessionResult | null> {
    const sessionPath = buildSessionPath(
      claudeProjectsPath,
      encodedDirName,
      sessionId,
    );

    return this.parseSessionByPath(sessionPath, sessionId);
  }

  /**
   * 通过完整路径解析会话文件
   *
   * @param sessionPath 会话文件完整路径
   * @param sessionId 会话 ID
   * @returns 解析后的消息实体列表
   */
  async parseSessionByPath(
    sessionPath: string,
    sessionId: string,
  ): Promise<ParseSessionResult | null> {
    // 读取所有消息（不分页）
    const result = await readSessionMessages(sessionPath, 999999, 0, 'asc');
    if (!result) {
      this.logger.warn(`无法读取会话文件: ${sessionPath}`);
      return null;
    }

    const messages = this.convertToEntities(result, sessionId);

    return {
      messages,
      total: result.total,
    };
  }

  /**
   * 将 shared-core 返回的消息转换为 MessageEntity
   */
  private convertToEntities(
    result: SessionMessagesResult,
    sessionId: string,
  ): MessageEntity[] {
    const entities: MessageEntity[] = [];

    for (const rawMessage of result.messages) {
      const entry = rawMessage as JsonlMessageEntry;
      const entity = this.convertSingleMessage(entry, sessionId);
      if (entity) {
        entities.push(entity);
      }
    }

    return entities;
  }

  /**
   * 转换单条消息
   */
  private convertSingleMessage(
    entry: JsonlMessageEntry,
    sessionId: string,
  ): MessageEntity | null {
    // 跳过 summary 类型的消息
    if (entry.type === 'summary') {
      return null;
    }

    // 获取消息类型
    const messageType = this.getMessageType(entry);
    if (!messageType) {
      return null;
    }

    // 获取消息内容
    const content = this.extractContent(entry);
    if (!content) {
      return null;
    }

    // 获取 UUID
    const uuid = entry.uuid || entry.message?.id;
    if (!uuid) {
      this.logger.debug('消息缺少 UUID，跳过');
      return null;
    }

    // 解析时间戳
    const timestamp = entry.timestamp ? new Date(entry.timestamp) : undefined;

    return new MessageEntity({
      uuid,
      sessionId,
      type: messageType,
      content,
      timestamp,
    });
  }

  /**
   * 获取消息类型
   */
  private getMessageType(entry: JsonlMessageEntry): MessageType | null {
    if (entry.type === 'user') {
      // 关键：过滤掉不应该显示的 user 消息
      if (!this.shouldDisplayUserMessage(entry)) {
        return null;
      }
      return MessageType.USER;
    }

    // assistant 消息有两种格式：
    // 1. type: 'message' + role: 'assistant'（旧格式）
    // 2. type: 'assistant'（新格式）
    if (entry.type === 'assistant' ||
        (entry.type === 'message' && entry.message?.role === 'assistant')) {
      return MessageType.ASSISTANT;
    }

    // 其他类型的消息不处理
    return null;
  }

  /**
   * 判断 User 消息是否应该显示
   *
   * 根据 /docs/17-user-message-scenarios.md：
   * - 82% 的 user 消息是 tool_result，不应该作为独立消息显示
   * - 只有约 18% 的 user 消息是真正的用户输入
   */
  private shouldDisplayUserMessage(entry: JsonlMessageEntry): boolean {
    // 1. 工具执行结果 - 不显示（占 80%）
    if (entry.toolUseResult) {
      return false;
    }

    // 2. 检查 content 中是否包含 tool_result
    if (this.hasToolResultInContent(entry)) {
      return false;
    }

    // 3. 仅 Transcript 可见 - 不显示
    if (entry.isVisibleInTranscriptOnly) {
      return false;
    }

    // 4. 压缩摘要 - 不显示
    if (entry.isCompactSummary) {
      return false;
    }

    // 5. 元数据消息 - 不显示
    if (entry.isMeta) {
      return false;
    }

    return true;
  }

  /**
   * 检查消息内容中是否包含 tool_result
   */
  private hasToolResultInContent(entry: JsonlMessageEntry): boolean {
    const content = entry.message?.content;
    if (!content || typeof content === 'string') {
      return false;
    }

    // content 是数组
    for (const block of content) {
      if (block.type === 'tool_result') {
        return true;
      }
    }

    return false;
  }

  /**
   * 提取消息内容
   *
   * 内容可能是字符串或数组，需要统一序列化为字符串存储
   */
  private extractContent(entry: JsonlMessageEntry): string | null {
    const message = entry.message;
    if (!message) {
      return null;
    }

    const content = message.content;
    if (!content) {
      return null;
    }

    // 如果是字符串，直接返回
    if (typeof content === 'string') {
      return content;
    }

    // 如果是数组（ContentBlock[]），提取文本内容
    if (Array.isArray(content)) {
      const textParts: string[] = [];

      for (const block of content) {
        if (block.type === 'text' && block.text) {
          textParts.push(block.text);
        }
      }

      // 如果没有提取到文本（只有 thinking/tool_use 等），跳过这条消息
      if (textParts.length === 0) {
        return null;
      }

      return textParts.join('\n');
    }

    // 其他情况序列化为 JSON
    return JSON.stringify(content);
  }
}
