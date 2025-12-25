import { Injectable, Logger } from '@nestjs/common';

/** 分片结果 */
export interface Chunk {
  /** 分片索引（从 0 开始） */
  index: number;
  /** 分片内容 */
  content: string;
  /** 分片类型 */
  type: 'code' | 'text';
}

/** 分片配置 */
interface ChunkingOptions {
  /** 最大分片长度（字符） */
  maxLength?: number;
  /** 重叠长度（字符） */
  overlap?: number;
  /** 最小分片长度（低于此值不单独成片） */
  minLength?: number;
}

const DEFAULT_OPTIONS: Required<ChunkingOptions> = {
  maxLength: 2000,
  overlap: 200,
  minLength: 100, // 降低阈值，避免过滤掉有意义的短段落
};

/**
 * 文本分片服务
 *
 * 职责：
 * - 将长文本分割成适合 embedding 的小块
 * - 保持代码块完整性
 * - 按语义边界（段落、标题）分割
 */
@Injectable()
export class ChunkingService {
  private readonly logger = new Logger(ChunkingService.name);

  /**
   * 对文本进行分片
   *
   * 策略：
   * 1. 短文本（<maxLength）不分片
   * 2. 提取代码块，保持完整
   * 3. 非代码部分按段落分割
   * 4. 超长段落按长度切分（带重叠）
   */
  chunk(content: string, options?: ChunkingOptions): Chunk[] {
    const opts = { ...DEFAULT_OPTIONS, ...options };

    // 空内容处理
    if (!content || content.trim().length === 0) {
      return [{ index: 0, content: 'empty', type: 'text' }];
    }

    // 短文本不分片
    if (content.length <= opts.maxLength) {
      return [{ index: 0, content: content.trim(), type: 'text' }];
    }

    // 分离代码块和文本
    const segments = this.separateCodeAndText(content);

    // 对每个 segment 进行分片
    const allChunks: Chunk[] = [];
    for (const segment of segments) {
      if (segment.type === 'code') {
        // 代码块：如果太长就按长度切，否则保持完整
        if (segment.content.length > opts.maxLength) {
          const codeChunks = this.splitByLength(segment.content, opts);
          allChunks.push(...codeChunks.map((c) => ({ ...c, type: 'code' as const })));
        } else if (segment.content.trim().length >= opts.minLength) {
          allChunks.push({ index: 0, content: segment.content.trim(), type: 'code' });
        }
      } else {
        // 文本：按段落分割，超长再按长度切
        const textChunks = this.splitTextByParagraph(segment.content, opts);
        allChunks.push(...textChunks);
      }
    }

    // 安全回退：如果所有 chunk 都被过滤掉了，保留完整内容
    if (allChunks.length === 0) {
      this.logger.warn(
        `分片结果为空，回退到完整内容 (length=${content.length})`,
      );
      return [{ index: 0, content: content.trim(), type: 'text' }];
    }

    // 重新编号
    return allChunks.map((chunk, index) => ({ ...chunk, index }));
  }

  /**
   * 分离代码块和普通文本
   */
  private separateCodeAndText(content: string): Array<{ type: 'code' | 'text'; content: string }> {
    const segments: Array<{ type: 'code' | 'text'; content: string }> = [];
    const codeBlockRegex = /```[\s\S]*?```/g;

    let lastIndex = 0;
    let match: RegExpExecArray | null;

    while ((match = codeBlockRegex.exec(content)) !== null) {
      // 代码块之前的文本
      if (match.index > lastIndex) {
        const textBefore = content.slice(lastIndex, match.index);
        if (textBefore.trim()) {
          segments.push({ type: 'text', content: textBefore });
        }
      }

      // 代码块本身
      segments.push({ type: 'code', content: match[0] });
      lastIndex = match.index + match[0].length;
    }

    // 最后一个代码块之后的文本
    if (lastIndex < content.length) {
      const textAfter = content.slice(lastIndex);
      if (textAfter.trim()) {
        segments.push({ type: 'text', content: textAfter });
      }
    }

    // 如果没有代码块，整个内容作为文本
    if (segments.length === 0) {
      segments.push({ type: 'text', content });
    }

    return segments;
  }

  /**
   * 按段落分割文本
   */
  private splitTextByParagraph(text: string, opts: Required<ChunkingOptions>): Chunk[] {
    const chunks: Chunk[] = [];

    // 按段落分割（双换行）
    const paragraphs = text.split(/\n\n+/).filter((p) => p.trim().length > 0);

    let currentChunk = '';

    for (const para of paragraphs) {
      const trimmedPara = para.trim();

      // 如果当前段落本身就超长，需要按长度切
      if (trimmedPara.length > opts.maxLength) {
        // 先保存之前累积的内容
        if (currentChunk.trim().length >= opts.minLength) {
          chunks.push({ index: 0, content: currentChunk.trim(), type: 'text' });
          currentChunk = '';
        }

        // 超长段落按长度切分
        const longChunks = this.splitByLength(trimmedPara, opts);
        chunks.push(...longChunks.map((c) => ({ ...c, type: 'text' as const })));
        continue;
      }

      // 检查加上这个段落后是否超长
      const combined = currentChunk ? `${currentChunk}\n\n${trimmedPara}` : trimmedPara;

      if (combined.length > opts.maxLength) {
        // 保存当前累积的内容
        if (currentChunk.trim().length >= opts.minLength) {
          chunks.push({ index: 0, content: currentChunk.trim(), type: 'text' });
        }
        currentChunk = trimmedPara;
      } else {
        currentChunk = combined;
      }
    }

    // 保存最后的内容
    if (currentChunk.trim().length >= opts.minLength) {
      chunks.push({ index: 0, content: currentChunk.trim(), type: 'text' });
    }

    return chunks;
  }

  /**
   * 按长度切分（带重叠）
   */
  private splitByLength(text: string, opts: Required<ChunkingOptions>): Chunk[] {
    const chunks: Chunk[] = [];
    let start = 0;

    while (start < text.length) {
      let end = Math.min(start + opts.maxLength, text.length);

      // 尝试在句子边界切分（找最近的句号、问号、感叹号、换行）
      if (end < text.length) {
        const searchStart = Math.max(start + opts.maxLength - 100, start);
        const searchEnd = end;
        const searchText = text.slice(searchStart, searchEnd);

        // 从后往前找句子边界
        const boundaries = ['\n', '。', '！', '？', '. ', '! ', '? '];
        let bestBoundary = -1;

        for (const boundary of boundaries) {
          const idx = searchText.lastIndexOf(boundary);
          if (idx > bestBoundary) {
            bestBoundary = idx;
          }
        }

        if (bestBoundary > 0) {
          end = searchStart + bestBoundary + 1;
        }
      }

      const chunk = text.slice(start, end).trim();
      if (chunk.length >= opts.minLength) {
        chunks.push({ index: 0, content: chunk, type: 'text' });
      }

      // 下一个起点，考虑重叠
      const prevStart = start;
      start = end - opts.overlap;
      // 防止无限循环：如果 start 没有前进，强制跳到 end
      if (start <= prevStart) {
        start = end;
      }
    }

    return chunks;
  }

  /**
   * 测试分片效果（用于调试）
   */
  testChunking(
    content: string,
    options?: ChunkingOptions,
  ): {
    originalLength: number;
    chunkCount: number;
    chunks: Array<{
      index: number;
      type: string;
      length: number;
      preview: string;
    }>;
  } {
    const chunks = this.chunk(content, options);

    return {
      originalLength: content.length,
      chunkCount: chunks.length,
      chunks: chunks.map((c) => ({
        index: c.index,
        type: c.type,
        length: c.content.length,
        preview: c.content.slice(0, 100) + (c.content.length > 100 ? '...' : ''),
      })),
    };
  }
}
