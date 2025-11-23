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
 */
interface JsonlMessageEntry {
  uuid: string;
  type: 'user' | 'message' | 'summary';
  message?: {
    id?: string;
    role?: 'user' | 'assistant';
    content?: string | ContentBlock[];
  };
  timestamp?: string;
}

/**
 * 内容块类型
 */
interface ContentBlock {
  type: string;
  text?: string;
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
      return MessageType.USER;
    }

    if (entry.type === 'message' && entry.message?.role === 'assistant') {
      return MessageType.ASSISTANT;
    }

    // 其他类型的消息不处理
    return null;
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

      // 如果没有提取到文本，返回 JSON 序列化的原始内容
      if (textParts.length === 0) {
        return JSON.stringify(content);
      }

      return textParts.join('\n');
    }

    // 其他情况序列化为 JSON
    return JSON.stringify(content);
  }
}
