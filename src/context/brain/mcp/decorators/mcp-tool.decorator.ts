import { SetMetadata } from '@nestjs/common';

export const MCP_TOOL_METADATA = 'mcp:tool';

export interface MCPToolOptions {
  name: string;
  description: string;
  inputSchema: {
    type: 'object';
    properties: Record<string, {
      type: string;
      description?: string;
      enum?: string[];
      items?: {
        type: string;
        properties?: Record<string, {
          type: string;
          description?: string;
        }>;
        required?: string[];
      };
      properties?: Record<string, unknown>;
      required?: string[];
    }>;
    required?: string[];
  };
  endpoint?: string; // 可选的自定义端点
}

/**
 * MCP 工具装饰器
 * 用于标记方法为 MCP 工具，自动注册到 MCP 服务器
 */
export function MCPTool(options: MCPToolOptions): MethodDecorator {
  return SetMetadata(MCP_TOOL_METADATA, options);
}
