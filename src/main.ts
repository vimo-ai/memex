import { NestFactory } from '@nestjs/core';
import { ValidationPipe, LogLevel } from '@nestjs/common';
import { AppModule } from './app.module';
import { MemexConfigService } from './config';

/**
 * 应用启动入口
 * Memex - Claude Code 会话历史管理系统
 */
async function bootstrap() {
  const logLevels = process.env.MEMEX_LOG_LEVELS
    ? (process.env.MEMEX_LOG_LEVELS.split(',').map((l) => l.trim()) as LogLevel[])
    : (['error', 'warn', 'log'] as LogLevel[]);

  const app = await NestFactory.create(AppModule, {
    logger: logLevels,
  });

  // 全局验证管道
  app.useGlobalPipes(
    new ValidationPipe({
      whitelist: true,
      transform: true,
    }),
  );

  // 全局前缀
  app.setGlobalPrefix('api');

  // 从配置服务获取端口
  const configService = app.get(MemexConfigService);
  const port = configService.port;

  await app.listen(port);

  console.log(`Memex 服务已启动: http://localhost:${port}`);
}

bootstrap();
