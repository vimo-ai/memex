import { Controller, Post, Body, Get, Query, Logger } from '@nestjs/common';
import { MCPRegistryService } from '../../mcp/services/mcp-registry.service';

interface MCPRequest {
  jsonrpc: '2.0';
  id: string | number;
  method: string;
  params?: unknown;
}

interface MCPResponse {
  jsonrpc: '2.0';
  id: string | number;
  result?: unknown;
  error?: {
    code: number;
    message: string;
    data?: unknown;
  };
}

/**
 * MCP HTTP 协议控制器
 * 实现完整的 MCP HTTP 传输协议
 *
 * 注意：Memex 是本地服务，不需要认证
 */
@Controller('mcp')
export class MCPController {
  private readonly logger = new Logger(MCPController.name);

  constructor(
    private readonly mcpRegistry: MCPRegistryService,
  ) {}

  @Post()
  async handleMCPRequest(
    @Body() request: MCPRequest,
  ): Promise<MCPResponse> {
    try {
      return await this.processMCPRequest(request);
    } catch (error) {
      return {
        jsonrpc: '2.0',
        id: request.id,
        error: {
          code: -32603,
          message: 'Internal error',
          data: (error as Error).message,
        },
      };
    }
  }

  @Get()
  async handleMCPGetRequest(
    @Query('method') method: string,
    @Query('id') id: string = '1',
  ): Promise<MCPResponse> {
    try {
      const mcpRequest: MCPRequest = {
        jsonrpc: '2.0',
        id: id || '1',
        method: method || 'tools/list',
      };
      return await this.processMCPRequest(mcpRequest);
    } catch (error) {
      return {
        jsonrpc: '2.0',
        id: id || '1',
        error: {
          code: -32603,
          message: 'Internal error',
          data: (error as Error).message,
        },
      };
    }
  }

  @Get('info')
  getMCPInfo() {
    const tools = this.mcpRegistry.getTools();

    return {
      server: {
        name: 'memex-mcp-server',
        version: '1.0.0',
        protocolVersion: '2024-11-05',
      },
      capabilities: {
        tools: {},
      },
      endpoints: {
        mcp: '/api/mcp',
        info: '/api/mcp/info',
      },
      tools: tools.map(tool => ({
        name: tool.name,
        description: tool.description,
        inputSchema: tool.inputSchema,
      })),
      usage: {
        post: 'Send MCP JSON-RPC requests to /api/mcp',
        get: 'Use query parameters: ?method=tools/list&id=1',
      },
    };
  }

  private async processMCPRequest(request: MCPRequest): Promise<MCPResponse> {
    const { method, params, id } = request;

    switch (method) {
      case 'initialize':
        return {
          jsonrpc: '2.0',
          id,
          result: {
            protocolVersion: '2024-11-05',
            capabilities: {
              tools: {},
            },
            serverInfo: {
              name: 'memex-mcp-server',
              version: '1.0.0',
            },
          },
        };

      case 'tools/list':
        const tools = this.mcpRegistry.getTools();
        return {
          jsonrpc: '2.0',
          id,
          result: {
            tools: tools.map(tool => ({
              name: tool.name,
              description: tool.description,
              inputSchema: tool.inputSchema,
            })),
          },
        };

      case 'tools/call':
        if (!params || typeof params !== 'object' || params === null) {
          return {
            jsonrpc: '2.0',
            id,
            error: {
              code: -32602,
              message: 'Invalid params for tools/call',
            },
          };
        }
        const { name, arguments: args } = params as { name: string; arguments: Record<string, unknown> };
        try {
          const result = await this.mcpRegistry.callTool(name, args);
          return {
            jsonrpc: '2.0',
            id,
            result: {
              content: [
                {
                  type: 'text',
                  text: typeof result === 'string' ? result : JSON.stringify(result, null, 2),
                },
              ],
            },
          };
        } catch (error) {
          this.logger.error(`MCP Tool execution failed for ${name}:`, error);
          return {
            jsonrpc: '2.0',
            id,
            error: {
              code: -32602,
              message: 'Tool execution failed',
              data: (error as Error).message,
            },
          };
        }

      default:
        return {
          jsonrpc: '2.0',
          id,
          error: {
            code: -32601,
            message: 'Method not found',
            data: `Unknown method: ${method}`,
          },
        };
    }
  }
}
