import { Controller, Post, Get, Body, Query } from '@nestjs/common';
import { RagService, RagResponse } from '../../application/services/rag.service';

/**
 * RAG 问答控制器
 *
 * 提供基于历史对话的问答接口
 */
@Controller('ask')
export class RagController {
  constructor(private readonly ragService: RagService) {}

  /**
   * POST /ask - 提交问题
   */
  @Post()
  async ask(
    @Body() body: { question: string; cwd?: string; contextWindow?: number; maxSources?: number },
  ): Promise<RagResponse> {
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
}
