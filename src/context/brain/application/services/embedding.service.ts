import { Injectable, Logger } from '@nestjs/common';

/** Ollama API 地址 */
const OLLAMA_API = process.env.OLLAMA_API || 'http://localhost:11434/api';

/** 默认 Embedding 模型 */
const DEFAULT_MODEL = 'bge-m3';

/** Embedding 向量维度 (bge-m3 = 1024) */
export const EMBEDDING_DIMENSION = 1024;

/** 最大文本长度（bge-m3 最大 8192 tokens，约 6000 中文字符，设置 4000 保留足够语义信息） */
const MAX_TEXT_LENGTH = 4000;

/**
 * Embedding 服务
 *
 * 职责：
 * - 调用 Ollama 生成文本向量
 * - 支持单条和批量处理
 * - 文本截断和预处理
 */
@Injectable()
export class EmbeddingService {
  private readonly logger = new Logger(EmbeddingService.name);
  private readonly model: string;

  constructor() {
    this.model = process.env.EMBEDDING_MODEL || DEFAULT_MODEL;
    this.logger.log(`Embedding 模型: ${this.model}`);
  }

  /**
   * 生成单条文本的 Embedding
   */
  async embed(text: string): Promise<number[]> {
    const truncated = this.truncateText(text);

    try {
      const response = await fetch(`${OLLAMA_API}/embeddings`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          model: this.model,
          prompt: truncated,
        }),
      });

      if (!response.ok) {
        throw new Error(
          `Ollama API 错误: ${response.status} ${response.statusText} (文本长度: ${truncated.length})`,
        );
      }

      const result = (await response.json()) as { embedding: number[] };
      return result.embedding;
    } catch (error) {
      this.logger.error(`Embedding 生成失败: ${error}`);
      throw error;
    }
  }

  /**
   * 批量生成 Embedding
   *
   * @param texts 文本数组
   * @param options.batchSize 批次大小，默认 10
   * @param options.onProgress 进度回调
   */
  async embedBatch(
    texts: string[],
    options?: {
      batchSize?: number;
      onProgress?: (processed: number, total: number) => void;
    },
  ): Promise<number[][]> {
    const { batchSize = 10, onProgress } = options || {};
    const results: number[][] = [];
    const total = texts.length;

    for (let i = 0; i < total; i += batchSize) {
      const batch = texts.slice(i, i + batchSize);

      // 并行处理当前批次
      const batchResults = await Promise.all(batch.map((text) => this.embed(text)));

      results.push(...batchResults);

      if (onProgress) {
        onProgress(Math.min(i + batchSize, total), total);
      }
    }

    return results;
  }

  /**
   * 检查 Ollama 服务是否可用
   */
  async isAvailable(): Promise<boolean> {
    try {
      const response = await fetch(`${OLLAMA_API.replace('/api', '')}/api/tags`, {
        method: 'GET',
        signal: AbortSignal.timeout(5000),
      });
      return response.ok;
    } catch {
      return false;
    }
  }

  /**
   * 检查指定模型是否已安装
   */
  async isModelAvailable(): Promise<boolean> {
    try {
      const response = await fetch(`${OLLAMA_API.replace('/api', '')}/api/tags`);
      if (!response.ok) return false;

      const data = (await response.json()) as { models: Array<{ name: string }> };
      return data.models.some((m) => m.name.startsWith(this.model));
    } catch {
      return false;
    }
  }

  /**
   * 预处理文本：截断过长内容，过滤无效字符
   */
  private truncateText(text: string): string {
    // 过滤空内容
    if (!text || text.trim().length === 0) {
      return 'empty';
    }

    let cleaned = text;

    // 1. 移除 ANSI 转义序列（终端颜色/格式代码）
    // eslint-disable-next-line no-control-regex
    cleaned = cleaned.replace(/\x1B\[[0-9;]*[a-zA-Z]/g, '');
    cleaned = cleaned.replace(/\x1B\]/g, '');

    // 2. 移除 NULL 字符
    cleaned = cleaned.replace(/\x00/g, '');

    // 3. 移除其他控制字符（保留换行、制表符）
    // eslint-disable-next-line no-control-regex
    cleaned = cleaned.replace(/[\x01-\x08\x0B\x0C\x0E-\x1F\x7F]/g, '');

    // 4. 移除损坏的 Unicode 字符（替换字符 U+FFFD 和无效序列）
    cleaned = cleaned.replace(/\uFFFD/g, '');

    // 5. 截断到最大长度
    if (cleaned.length > MAX_TEXT_LENGTH) {
      cleaned = cleaned.slice(0, MAX_TEXT_LENGTH);
    }

    // 如果清理后为空，返回占位符
    if (cleaned.trim().length === 0) {
      return 'empty';
    }

    return cleaned;
  }

  /**
   * 获取当前使用的模型名称
   */
  getModelName(): string {
    return this.model;
  }

  /**
   * 卸载模型释放 GPU buffer，然后等待重新加载
   * 用于避免 Metal buffer 累积导致的崩溃
   */
  async unloadModel(): Promise<void> {
    try {
      // 1. 卸载模型
      await fetch(`${OLLAMA_API}/embeddings`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          model: this.model,
          prompt: '',
          keep_alive: 0,
        }),
      });
      this.logger.log('已卸载模型，等待重新加载...');

      // 2. 发探测请求，等模型重新加载完成
      await this.embed('ping');

      // 3. 额外等待 5 秒，让 GPU 完全准备好处理并发请求
      this.logger.log('模型已加载，等待 5 秒预热...');
      await new Promise((resolve) => setTimeout(resolve, 5000));
      this.logger.log('模型预热完成，继续处理');
    } catch (error) {
      this.logger.warn(`卸载/重载模型失败: ${error}`);
    }
  }
}
