import { Injectable, Inject, Logger } from '@nestjs/common';
import { HybridSearchService } from './hybrid-search.service';
import {
  ISessionRepository,
  SESSION_REPOSITORY,
} from '../../domain/repositories/session.repository.interface';
import { MemexConfigService } from '../../../../config/memex-config.service';

/**
 * RAG 响应结果
 */
export interface RagResponse {
  /** 生成的答案 */
  answer: string;
  /** 引用的来源 */
  sources: Array<{
    sessionId: string;
    projectName: string;
    messageIndex: number;
    snippet: string;
    score: number;
  }>;
  /** 使用的模型 */
  model: string;
  /** 消耗的 token 数（如果可用） */
  tokensUsed?: number;
}

/**
 * RAG 查询选项
 */
export interface RagOptions {
  /** 用户问题 */
  question: string;
  /** 当前工作目录，用于匹配项目过滤 */
  cwd?: string;
  /** 前后消息数，用于提供上下文，默认 3 */
  contextWindow?: number;
  /** 最大引用数，默认 5 */
  maxSources?: number;
}

/**
 * RAG (Retrieval Augmented Generation) 服务
 *
 * 职责：
 * - 基于历史对话提供问答能力
 * - 使用混合检索获取相关消息
 * - 构建上下文 prompt
 * - 调用 Ollama chat API 生成答案
 */
@Injectable()
export class RagService {
  private readonly logger = new Logger(RagService.name);

  constructor(
    private readonly hybridSearchService: HybridSearchService,
    @Inject(SESSION_REPOSITORY)
    private readonly sessionRepository: ISessionRepository,
    private readonly configService: MemexConfigService,
  ) {}

  /**
   * 基于历史对话回答问题
   */
  async ask(options: RagOptions): Promise<RagResponse> {
    const { question, cwd, contextWindow = 3, maxSources = 5 } = options;

    this.logger.log(`[RAG] 收到问题: "${question}"`);

    // 1. 使用混合检索获取相关消息
    const searchResults = await this.hybridSearchService.search({
      query: question,
      mode: 'hybrid',
      limit: maxSources,
    });

    if (searchResults.length === 0) {
      return {
        answer: '抱歉，我在历史对话中没有找到相关信息。',
        sources: [],
        model: this.configService.chatModel,
      };
    }

    this.logger.log(`[RAG] 检索到 ${searchResults.length} 条相关消息`);

    // 2. 为每条消息构建上下文（拉取前后消息）
    const sourcesWithContext = searchResults.map((result) => {
      const allMessages = this.sessionRepository.findMessagesBySessionId(result.sessionId);
      const messageIndex = allMessages.findIndex((msg) => msg.uuid === result.uuid);

      if (messageIndex === -1) {
        this.logger.warn(`[RAG] 未找到消息 ${result.uuid} 在会话 ${result.sessionId} 中的索引`);
        return {
          sessionId: result.sessionId,
          projectName: result.projectName,
          messageIndex: 0,
          snippet: result.snippet || result.content.slice(0, 200),
          score: result.score,
          contextMessages: [result.content],
        };
      }

      // 计算上下文窗口范围
      const startIdx = Math.max(0, messageIndex - contextWindow);
      const endIdx = Math.min(allMessages.length, messageIndex + contextWindow + 1);
      const contextMessages = allMessages.slice(startIdx, endIdx);

      return {
        sessionId: result.sessionId,
        projectName: result.projectName,
        messageIndex,
        snippet: result.snippet || result.content.slice(0, 200),
        score: result.score,
        contextMessages: contextMessages.map(
          (msg) => `[${msg.type}] ${msg.content.slice(0, 500)}`,
        ),
      };
    });

    // 3. 构建 prompt
    const prompt = this.buildPrompt(question, sourcesWithContext);

    // 4. 调用 Ollama chat API
    let answer: string;
    let tokensUsed: number | undefined;

    try {
      const response = await this.callOllamaChat(prompt);
      answer = response.answer;
      tokensUsed = response.tokensUsed;
    } catch (error) {
      this.logger.error(`[RAG] Ollama 调用失败: ${error}`);
      return {
        answer: `抱歉，生成答案时出现错误: ${error instanceof Error ? error.message : '未知错误'}`,
        sources: sourcesWithContext.map((s) => ({
          sessionId: s.sessionId,
          projectName: s.projectName,
          messageIndex: s.messageIndex,
          snippet: s.snippet,
          score: s.score,
        })),
        model: this.configService.chatModel,
      };
    }

    // 5. 返回结果
    return {
      answer,
      sources: sourcesWithContext.map((s) => ({
        sessionId: s.sessionId,
        projectName: s.projectName,
        messageIndex: s.messageIndex,
        snippet: s.snippet,
        score: s.score,
      })),
      model: this.configService.chatModel,
      tokensUsed,
    };
  }

  /**
   * 构建 prompt
   */
  private buildPrompt(
    question: string,
    sources: Array<{
      sessionId: string;
      projectName: string;
      messageIndex: number;
      snippet: string;
      score: number;
      contextMessages: string[];
    }>,
  ): string {
    const systemPrompt = `你是一个知识助手，基于用户的历史 Claude Code 对话记录回答问题。

以下是相关的历史对话片段：

${sources
  .map(
    (source, idx) => `---
[来源 ${idx + 1}: 项目 ${source.projectName}, 会话 ${source.sessionId.slice(0, 8)}...]
${source.contextMessages.join('\n')}
---`,
  )
  .join('\n\n')}

请基于以上历史记录回答用户的问题。如果历史记录中没有相关信息，请如实说明。`;

    return `${systemPrompt}\n\n用户问题：${question}`;
  }

  /**
   * 调用 Ollama chat API
   */
  private async callOllamaChat(prompt: string): Promise<{ answer: string; tokensUsed?: number }> {
    const ollamaApi = this.configService.ollamaApi;
    const chatModel = this.configService.chatModel;

    this.logger.log(`[RAG] 调用 Ollama: ${ollamaApi}/chat, model: ${chatModel}`);

    const response = await fetch(`${ollamaApi.replace(/\/api$/, '')}/api/chat`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({
        model: chatModel,
        messages: [
          {
            role: 'user',
            content: prompt,
          },
        ],
        stream: false,
      }),
    });

    if (!response.ok) {
      const errorText = await response.text();
      throw new Error(`Ollama API 返回错误: ${response.status} ${errorText}`);
    }

    const data = await response.json();

    return {
      answer: data.message?.content || '无法生成答案',
      tokensUsed: data.eval_count,
    };
  }
}
