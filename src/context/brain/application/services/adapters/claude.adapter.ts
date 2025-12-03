import { Injectable, Logger } from '@nestjs/common';
import { promises as fs } from 'fs';
import { buildSessionPath, scanProjects, scanSessions } from '@vimo-ai/vlaude-shared-core';
import { MemexConfigService } from '../../../../../config';
import { ParserService, type ParseSessionResult } from '../parser.service';
import {
  AdapterParseResult,
  AdapterSessionMeta,
  ConversationAdapter,
} from './conversation-adapter.interface';

/**
 * Claude Code 数据适配器
 * 包装现有的 shared-core 扫描与 ParserService 解析逻辑
 */
@Injectable()
export class ClaudeAdapter implements ConversationAdapter {
  readonly source = 'claude';
  private readonly logger = new Logger(ClaudeAdapter.name);

  constructor(
    private readonly configService: MemexConfigService,
    private readonly parserService: ParserService,
  ) {}

  /**
   * 列出所有 Claude 会话元数据
   */
  async listSessions(): Promise<AdapterSessionMeta[]> {
    const claudeProjectsPath = this.configService.claudeProjectsPath;
    const projects = await scanProjects(claudeProjectsPath);
    const results: AdapterSessionMeta[] = [];

    for (const projectInfo of projects) {
      const sessions = await scanSessions(
        claudeProjectsPath,
        projectInfo.encodedDirName,
        projectInfo.path,
      );

      for (const sessionMeta of sessions) {
        const sessionPath = buildSessionPath(
          claudeProjectsPath,
          projectInfo.encodedDirName,
          sessionMeta.id,
        );

        let fileMtime: number | undefined;
        let fileSize: number | undefined;

        try {
          const stat = await fs.stat(sessionPath);
          fileMtime = stat.mtimeMs;
          fileSize = stat.size;
        } catch (error) {
          this.logger.warn(`stat 会话文件失败: ${sessionPath}`, error as Error);
        }

        results.push({
          id: sessionMeta.id,
          source: this.source,
          channel: 'code',
          projectPath: projectInfo.path,
          projectName: projectInfo.name,
          encodedDirName: projectInfo.encodedDirName,
          sessionPath,
          fileMtime,
          fileSize,
          createdAt: sessionMeta.createdAt,
          updatedAt: sessionMeta.lastUpdated,
          meta: {
            messageCount: sessionMeta.messageCount,
          },
        });
      }
    }

    return results;
  }

  /**
   * 解析单个会话
   */
  async parseSession(meta: AdapterSessionMeta): Promise<AdapterParseResult | null> {
    if (!meta.sessionPath) {
      this.logger.warn(`缺少 sessionPath，无法解析 claude 会话: ${meta.id}`);
      return null;
    }

    const parsed = await this.parserService.parseSessionByPath(
      meta.sessionPath,
      meta.id,
    );

    if (!parsed) {
      return null;
    }

    return this.decorateMessages(parsed);
  }

  /**
   * 为 Claude 消息增加来源和渠道标记，并提取会话元数据
   */
  private decorateMessages(parsed: ParseSessionResult): AdapterParseResult {
    const messages = parsed.messages.map((msg) => {
      msg.source = this.source;
      msg.channel = msg.channel ?? 'code';
      return msg;
    });

    return {
      messages,
      createdAt: messages[0]?.timestamp,
      updatedAt: messages[messages.length - 1]?.timestamp,
      cwd: parsed.cwd,
      model: parsed.model,
      meta: {
        total: parsed.total,
      },
    };
  }
}
