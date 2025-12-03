import { Injectable, Logger } from '@nestjs/common';
import { promises as fs } from 'fs';
import { join } from 'path';
import { randomUUID } from 'crypto';
import { MemexConfigService } from '../../../../../config';
import { MessageEntity, MessageType } from '../../../domain/entities/message.entity';
import {
  AdapterParseResult,
  AdapterSessionMeta,
  ConversationAdapter,
} from './conversation-adapter.interface';

interface CodexHistoryEntry {
  session_id: string;
  ts: number | string;
  text: string;
}

/**
 * Codex CLI 数据适配器
 * 解析 history.jsonl 摘要和 rollout 事件流
 */
@Injectable()
export class CodexAdapter implements ConversationAdapter {
  readonly source = 'codex';
  private readonly logger = new Logger(CodexAdapter.name);

  constructor(private readonly configService: MemexConfigService) {}

  /**
   * 列出 Codex 摘要中的所有会话
   */
  async listSessions(): Promise<AdapterSessionMeta[]> {
    const historyPath = this.configService.codexHistoryPath;
    const sessionsRoot = this.configService.codexSessionsRoot;
    const content = await this.safeReadFile(historyPath);
    if (!content) return [];

    const lines = content.split('\n').filter((line) => line.trim().length > 0);
    const metas: AdapterSessionMeta[] = [];

    for (const line of lines) {
      const entry = this.safeParseJson<CodexHistoryEntry>(line);
      if (!entry || !entry.session_id) continue;

      const tsDate = this.parseTimestamp(entry.ts);
      const sessionPath =
        (await this.resolveSessionPath(sessionsRoot, tsDate, entry.session_id)) || undefined;
      const { fileMtime, fileSize } = sessionPath ? await this.safeStat(sessionPath) : {};

      // 从 rollout 文件第一行提取 session_meta.cwd 作为 projectPath
      const projectPath = sessionPath
        ? await this.extractProjectPath(sessionPath)
        : sessionsRoot;

      metas.push({
        id: entry.session_id,
        source: this.source,
        channel: 'cli',
        projectPath: projectPath || sessionsRoot,
        sessionPath,
        fileMtime,
        fileSize,
        createdAt: tsDate ?? undefined,
        updatedAt: tsDate ?? undefined,
        meta: {
          historyText: entry.text,
          historyTs: entry.ts,
        },
      });
    }

    return metas;
  }

  /**
   * 从 rollout 文件提取 projectPath (cwd)
   * 只读取前几行，找到 session_meta 事件获取 cwd
   */
  private async extractProjectPath(sessionPath: string): Promise<string | null> {
    try {
      const content = await fs.readFile(sessionPath, 'utf-8');
      // 只检查前 5 行，session_meta 通常在第一行
      const lines = content.split('\n').slice(0, 5);
      for (const line of lines) {
        if (!line.trim()) continue;
        const raw = this.safeParseJson<any>(line);
        if (!raw) continue;

        const eventType = raw.type || raw.payload?.type;
        if (eventType === 'session_meta') {
          const payload = raw.payload ?? raw;
          return payload.cwd ?? null;
        }
      }
    } catch {
      // 文件读取失败，返回 null
    }
    return null;
  }

  /**
   * 解析 Codex 会话事件流
   */
  async parseSession(meta: AdapterSessionMeta): Promise<AdapterParseResult | null> {
    if (!meta.sessionPath) {
      this.logger.warn(`缺少 sessionPath，无法解析 codex 会话: ${meta.id}`);
      return null;
    }

    const content = await this.safeReadFile(meta.sessionPath);
    if (!content) return null;

    const messages: MessageEntity[] = [];
    let createdAt = meta.createdAt;
    let updatedAt = meta.updatedAt;
    let cwd = meta.cwd;
    let model = meta.model;
    const metaBag: Record<string, any> = meta.meta ? { ...meta.meta } : {};

    // 首条用户消息来自 history 摘要
    if (metaBag.historyText) {
      messages.push(
        new MessageEntity({
          uuid: `${meta.id}:user:0`,
          sessionId: meta.id,
          type: MessageType.USER,
          content: metaBag.historyText,
          timestamp: this.parseTimestamp(metaBag.historyTs) ?? createdAt,
          source: this.source,
          channel: meta.channel ?? 'cli',
        }),
      );
    }

    for (const line of content.split('\n')) {
      if (!line.trim()) continue;
      const raw = this.safeParseJson<any>(line);
      if (!raw) {
        this.pushError(metaBag, 'bad_json_line', line);
        continue;
      }

      const event = raw.payload ?? raw;
      const eventType = raw.type || event.type;

      if (eventType === 'session_meta') {
        const sessionMeta = event.session_meta || event;
        cwd = cwd ?? sessionMeta.cwd;
        model = model ?? sessionMeta.model;
        createdAt = createdAt ?? this.parseTimestamp(sessionMeta.ts);
        updatedAt = updatedAt ?? this.parseTimestamp(sessionMeta.ts);
        metaBag.session_meta = sessionMeta;
        continue;
      }

      if (eventType === 'response_item') {
        const item = event.response_item || event;
        const itemType = item.type || item.kind || item.message?.type;
        if (itemType === 'message' || item.message) {
          const contentText =
            typeof item.message === 'string'
              ? item.message
              : item.message?.text || item.message?.content || JSON.stringify(item.message);
          messages.push(
            new MessageEntity({
              uuid: item.id || randomUUID(),
              sessionId: meta.id,
              type: MessageType.ASSISTANT,
              content: contentText || '',
              timestamp: this.parseTimestamp(item.ts) ?? updatedAt,
              source: this.source,
              channel: meta.channel ?? 'cli',
              model: item.model ?? model,
              raw: item.reasoning ? JSON.stringify(item.reasoning) : undefined,
            }),
          );
          updatedAt = messages[messages.length - 1].timestamp ?? updatedAt;
          continue;
        }

        if (itemType === 'reasoning' || item.reasoning) {
          const reasoningText =
            item.reasoning?.content || item.reasoning?.text || JSON.stringify(item.reasoning);
          messages.push(
            new MessageEntity({
              uuid: item.id || randomUUID(),
              sessionId: meta.id,
              type: MessageType.ASSISTANT,
              content: reasoningText || '[reasoning]',
              raw: reasoningText,
              timestamp: this.parseTimestamp(item.ts) ?? updatedAt,
              source: this.source,
              channel: meta.channel ?? 'cli',
              model: item.model ?? model,
            }),
          );
          updatedAt = messages[messages.length - 1].timestamp ?? updatedAt;
          continue;
        }

        if (itemType === 'function_call' || itemType === 'custom_tool_call' || item.tool_call) {
          const tool = item.tool_call || item.function_call || item.custom_tool_call || item;
          messages.push(
            new MessageEntity({
              uuid: tool.id || randomUUID(),
              sessionId: meta.id,
              type: MessageType.ASSISTANT,
              content: `Tool call: ${tool.name || tool.function_name || 'unknown'}`,
              timestamp: this.parseTimestamp(tool.ts) ?? updatedAt,
              source: this.source,
              channel: meta.channel ?? 'cli',
              model: item.model ?? model,
              toolCallId: tool.id,
              toolName: tool.name || tool.function_name,
              toolArgs: tool.arguments ? JSON.stringify(tool.arguments) : undefined,
              raw: JSON.stringify(tool),
            }),
          );
          updatedAt = messages[messages.length - 1].timestamp ?? updatedAt;
          continue;
        }
      }

      if (eventType === 'tool_output' || eventType === 'tool_call_output' || event.tool_output || event.tool_call_output || event.output) {
        const toolOutput = event.tool_output || event.tool_call_output || event.output || event;
        messages.push(
          new MessageEntity({
            uuid: toolOutput.id || randomUUID(),
            sessionId: meta.id,
            type: MessageType.TOOL,
            content:
              typeof toolOutput === 'string'
                ? toolOutput
                : toolOutput.text || JSON.stringify(toolOutput),
            timestamp: this.parseTimestamp(toolOutput.ts) ?? updatedAt,
            source: this.source,
            channel: meta.channel ?? 'cli',
            toolCallId: toolOutput.call_id || toolOutput.tool_call_id,
            raw: typeof toolOutput === 'string' ? undefined : JSON.stringify(toolOutput),
          }),
        );
        updatedAt = messages[messages.length - 1].timestamp ?? updatedAt;
        continue;
      }

      if (event.event_msg) {
        metaBag.event_msgs = metaBag.event_msgs || [];
        metaBag.event_msgs.push(event.event_msg);
        continue;
      }
    }

    return {
      messages,
      createdAt,
      updatedAt,
      cwd,
      model,
      meta: metaBag,
    };
  }

  private parseTimestamp(ts: number | string | undefined): Date | undefined {
    if (ts === undefined || ts === null) return undefined;
    const num = typeof ts === 'string' ? Number(ts) : ts;
    if (Number.isNaN(num)) return undefined;
    // Codex ts 可能是秒或毫秒，简单判定
    const value = num > 1e12 ? num : num * 1000;
    const d = new Date(value);
    return Number.isNaN(d.getTime()) ? undefined : d;
  }

  /**
   * 根据 ts 和 sessionId 尝试定位 rollout 文件
   * - 优先在对应日期目录查找包含 sessionId 的文件名
   * - 找不到则返回 null
   */
  private async resolveSessionPath(
    root: string,
    ts: Date | undefined,
    sessionId: string,
  ): Promise<string | null> {
    const date = ts ?? new Date();
    const year = date.getFullYear();
    const month = `${date.getMonth() + 1}`.padStart(2, '0');
    const day = `${date.getDate()}`.padStart(2, '0');
    const dayDir = join(root, `${year}`, `${month}`, `${day}`);

    try {
      const files = await fs.readdir(dayDir);
      const matched = files.find((f) => f.includes(sessionId));
      if (matched) {
        return join(dayDir, matched);
      }
    } catch {
      // 目录不存在或无法读取
    }

    // 全局兜底：递归查找包含 sessionId 的文件
    const fallback = await this.searchSessionFileRecursive(root, sessionId);
    if (fallback) {
      return fallback;
    }

    return null;
  }

  /**
   * 在 root 下递归查找包含 sessionId 的文件，返回找到的第一个路径
   */
  private async searchSessionFileRecursive(root: string, sessionId: string): Promise<string | null> {
    const stack = [root];

    while (stack.length > 0) {
      const dir = stack.pop()!;
      let entries: string[] = [];
      try {
        entries = await fs.readdir(dir);
      } catch {
        continue;
      }

      for (const entry of entries) {
        const full = join(dir, entry);
        if (entry.includes(sessionId)) {
          return full;
        }
        // 简单判断目录
        if (!entry.includes('.') || entry.length === 4 || entry.length === 2) {
          stack.push(full);
        }
      }
    }

    return null;
  }

  private async safeReadFile(path: string): Promise<string | null> {
    try {
      return await fs.readFile(path, 'utf-8');
    } catch (error) {
      this.logger.warn(`读取文件失败: ${path}`, error as Error);
      return null;
    }
  }

  private async safeStat(path: string): Promise<{ fileMtime?: number; fileSize?: number }> {
    try {
      const stat = await fs.stat(path);
      return { fileMtime: stat.mtimeMs, fileSize: stat.size };
    } catch {
      return {};
    }
  }

  private safeParseJson<T>(line: string): T | null {
    try {
      return JSON.parse(line) as T;
    } catch {
      return null;
    }
  }

  private pushError(meta: Record<string, any>, type: string, payload: unknown): void {
    meta.errors = meta.errors || [];
    meta.errors.push({ type, payload });
  }
}
