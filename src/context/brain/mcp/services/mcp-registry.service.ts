import { Injectable, OnModuleInit } from '@nestjs/common';
import { DiscoveryService, MetadataScanner, Reflector } from '@nestjs/core';
import { MCP_TOOL_METADATA, MCPToolOptions } from '../decorators/mcp-tool.decorator';

export interface RegisteredTool {
  name: string;
  description: string;
  inputSchema: Record<string, unknown>;
  endpoint: string;
  instance: Record<string, unknown>;
  methodName: string;
  parameterNames?: string[]; // 保存参数名称顺序
}

/**
 * MCP 工具注册服务
 * 自动发现并注册带有 @MCPTool 装饰器的方法
 */
@Injectable()
export class MCPRegistryService implements OnModuleInit {
  private tools: Map<string, RegisteredTool> = new Map();

  constructor(
    private readonly discoveryService: DiscoveryService,
    private readonly metadataScanner: MetadataScanner,
    private readonly reflector: Reflector,
  ) {}

  async onModuleInit() {
    await this.discoverMCPTools();
  }

  /**
   * 发现所有标记了 @MCPTool 的方法
   */
  private async discoverMCPTools() {
    const providers = this.discoveryService.getProviders();

    for (const wrapper of providers) {
      const { instance } = wrapper;
      if (!instance || typeof instance !== 'object') {
        continue;
      }

      const prototype = Object.getPrototypeOf(instance);
      const methodNames = this.metadataScanner.scanFromPrototype(
        instance,
        prototype,
        (name) => name,
      );

      for (const methodName of methodNames) {
        const metadata = this.reflector.get<MCPToolOptions>(
          MCP_TOOL_METADATA,
          instance[methodName],
        );

        if (metadata) {
          const endpoint = metadata.endpoint || `/mcp/tools/${metadata.name}`;

          // 从 inputSchema 提取参数名称顺序
          const parameterNames = Object.keys(metadata.inputSchema.properties || {});

          const tool: RegisteredTool = {
            name: metadata.name,
            description: metadata.description,
            inputSchema: metadata.inputSchema,
            endpoint,
            instance,
            methodName,
            parameterNames,
          };

          this.tools.set(metadata.name, tool);
          console.log(`Registered MCP tool: ${metadata.name} -> ${endpoint}`);
        }
      }
    }
  }

  /**
   * 获取所有注册的工具
   */
  getTools(): RegisteredTool[] {
    return Array.from(this.tools.values());
  }

  /**
   * 根据名称获取工具
   */
  getTool(name: string): RegisteredTool | undefined {
    return this.tools.get(name);
  }

  /**
   * 调用工具方法
   * @param name 工具名称
   * @param args 工具参数
   */
  async callTool(
    name: string,
    args: Record<string, unknown>,
  ): Promise<unknown> {
    const tool = this.getTool(name);
    if (!tool) {
      throw new Error(`Tool not found: ${name}`);
    }

    try {
      // 按照参数名称顺序构建参数数组
      let orderedArgs: unknown[];

      if (tool.parameterNames && tool.parameterNames.length > 0) {
        // 如果有参数名称顺序信息，按照顺序传递参数
        orderedArgs = tool.parameterNames.map(paramName => args[paramName]);
      } else {
        // 降级处理：如果没有参数顺序信息，使用原来的方式
        orderedArgs = Object.values(args);
      }

      const methodToCall = tool.instance[tool.methodName];
      if (typeof methodToCall !== 'function') {
        throw new Error(`Method ${tool.methodName} is not a function`);
      }
      const result = await methodToCall.call(tool.instance, ...orderedArgs);
      return result;
    } catch (error) {
      console.error(`Error calling tool ${name}:`, error);
      throw error;
    }
  }
}
