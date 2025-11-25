import { Controller, Get, Param, Query, NotFoundException } from '@nestjs/common';
import { Inject } from '@nestjs/common';
import {
  ISessionRepository,
  SESSION_REPOSITORY,
} from '../../domain/repositories/session.repository.interface';
import {
  SessionResponseDto,
  MessageResponseDto,
  MessageListResponseDto,
  SessionSearchResponseDto,
} from '../dto/session.dto';
import { SessionEntity } from '../../domain/entities/session.entity';
import { MessageEntity } from '../../domain/entities/message.entity';

/**
 * 会话控制器
 *
 * 提供会话相关的 HTTP API
 */
@Controller('sessions')
export class SessionController {
  constructor(
    @Inject(SESSION_REPOSITORY)
    private readonly sessionRepository: ISessionRepository,
  ) {}

  /**
   * 根据 ID 前缀搜索会话
   *
   * GET /api/sessions/search?idPrefix=xxx&limit=20
   */
  @Get('search')
  searchSessions(
    @Query('idPrefix') idPrefix: string,
    @Query('limit') limitStr?: string,
  ): SessionSearchResponseDto {
    if (!idPrefix || idPrefix.trim().length === 0) {
      return {
        query: '',
        total: 0,
        sessions: [],
      };
    }

    const limit = limitStr ? parseInt(limitStr, 10) : 20;
    const sessions = this.sessionRepository.searchSessionsByIdPrefix(
      idPrefix.trim(),
      limit,
    );

    return {
      query: idPrefix,
      total: sessions.length,
      sessions: sessions.map((s) => this.toSessionDto(s)),
    };
  }

  /**
   * 获取会话详情
   *
   * GET /api/sessions/:id
   */
  @Get(':id')
  getSession(@Param('id') id: string): SessionResponseDto {
    const session = this.sessionRepository.findSessionById(id);

    if (!session) {
      throw new NotFoundException(`会话不存在: ${id}`);
    }

    return this.toSessionDto(session);
  }

  /**
   * 获取会话的消息列表
   *
   * GET /api/sessions/:id/messages
   */
  @Get(':id/messages')
  getSessionMessages(@Param('id') id: string): MessageListResponseDto {
    const session = this.sessionRepository.findSessionById(id);

    if (!session) {
      throw new NotFoundException(`会话不存在: ${id}`);
    }

    const messages = this.sessionRepository.findMessagesBySessionId(id);

    return {
      total: messages.length,
      messages: messages.map((m) => this.toMessageDto(m)),
    };
  }

  /**
   * 会话实体转 DTO
   */
  private toSessionDto(session: SessionEntity): SessionResponseDto {
    return {
      id: session.id,
      projectId: session.projectId,
      status: session.status,
      messageCount: session.messageCount,
      createdAt: session.createdAt?.toISOString() ?? new Date().toISOString(),
      updatedAt: session.updatedAt?.toISOString() ?? new Date().toISOString(),
    };
  }

  /**
   * 消息实体转 DTO
   */
  private toMessageDto(message: MessageEntity): MessageResponseDto {
    return {
      id: message.id!,
      uuid: message.uuid,
      sessionId: message.sessionId,
      type: message.type,
      content: message.content,
      timestamp: message.timestamp?.toISOString(),
      createdAt: message.createdAt?.toISOString() ?? new Date().toISOString(),
    };
  }
}
