import { NestFactory } from '@nestjs/core';
import { ValidationPipe } from '@nestjs/common';
import { AppModule } from './app.module';

/**
 * 应用启动入口
 * Memex - Claude Code 会话历史管理系统
 */
async function bootstrap() {
  const app = await NestFactory.create(AppModule);

  // 全局验证管道
  app.useGlobalPipes(
    new ValidationPipe({
      whitelist: true,
      transform: true,
    }),
  );

  // 全局前缀
  app.setGlobalPrefix('api');

  const port = process.env.PORT ?? 10013;
  await app.listen(port);

  console.log(`Memex 服务已启动: http://localhost:${port}`);
}

bootstrap();
