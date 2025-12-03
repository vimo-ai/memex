import { Injectable } from '@nestjs/common';
import { ConversationAdapter } from './conversation-adapter.interface';
import { ClaudeAdapter } from './claude.adapter';
import { CodexAdapter } from './codex.adapter';

/**
 * 适配器注册表
 */
@Injectable()
export class AdapterRegistryService {
  constructor(
    private readonly claudeAdapter: ClaudeAdapter,
    private readonly codexAdapter: CodexAdapter,
  ) {}

  /**
   * 返回可用的会话适配器列表
   */
  getAdapters(): ConversationAdapter[] {
    return [this.claudeAdapter, this.codexAdapter];
  }
}
