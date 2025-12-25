import { Controller, Post, Get, Body, Query, NotImplementedException } from '@nestjs/common';
import { RagService, RagResponse } from '../../application/services/rag.service';
import { MemexConfigService } from '../../../../config/memex-config.service';

/**
 * RAG 问答控制器
 *
 * 提供基于历史对话的问答接口
 * 需要启用 ENABLE_RAG=true 才能使用
 */
@Controller('ask')
export class RagController {
  constructor(
    private readonly ragService: RagService,
    private readonly configService: MemexConfigService,
  ) {}

  /**
   * 检查 RAG 是否启用
   */
  private checkRagEnabled(): void {
    if (!this.configService.enableRag) {
      throw new NotImplementedException(
        'RAG 功能未启用。请设置环境变量 ENABLE_RAG=true 并确保 Ollama 已安装 chat 模型。',
      );
    }
  }

  /**
   * POST /ask - 提交问题
   */
  @Post()
  async ask(
    @Body() body: { question: string; cwd?: string; contextWindow?: number; maxSources?: number },
  ): Promise<RagResponse> {
    this.checkRagEnabled();
    return this.ragService.ask({
      question: body.question,
      cwd: body.cwd,
      contextWindow: body.contextWindow,
      maxSources: body.maxSources,
    });
  }

  /**
   * GET /ask?q=xxx - 快捷查询（适合浏览器测试）
   */
  @Get()
  async askGet(
    @Query('q') question: string,
    @Query('cwd') cwd?: string,
    @Query('contextWindow') contextWindow?: string,
    @Query('maxSources') maxSources?: string,
  ): Promise<RagResponse> {
    this.checkRagEnabled();
    if (!question) {
      throw new Error('参数 q (question) 是必需的');
    }

    return this.ragService.ask({
      question,
      cwd,
      contextWindow: contextWindow ? parseInt(contextWindow, 10) : undefined,
      maxSources: maxSources ? parseInt(maxSources, 10) : undefined,
    });
  }

  /**
   * GET /ask/status - 检查 RAG 功能状态
   */
  @Get('status')
  async getStatus(): Promise<{ enabled: boolean; chatModel: string }> {
    return {
      enabled: this.configService.enableRag,
      chatModel: this.configService.chatModel,
    };
  }
}
