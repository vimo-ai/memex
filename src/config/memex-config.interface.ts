/**
 * Memex 配置接口定义
 */
export interface MemexConfig {
  // 服务配置
  /** HTTP 服务端口 */
  port: number;

  // 数据目录
  /** 主数据目录 */
  dataDir: string;
  /** 备份目录 */
  backupDir: string;

  // Claude Code 配置
  /** Claude Code 项目目录路径 */
  claudeProjectsPath: string;

  // Ollama 配置
  /** Ollama API 地址 */
  ollamaApi: string;
  /** 嵌入模型名称 */
  embeddingModel: string;
  /** 聊天模型名称 */
  chatModel: string;
}
