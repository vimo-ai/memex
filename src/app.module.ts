import { Module } from '@nestjs/common';
import { ServeStaticModule } from '@nestjs/serve-static';
import { join } from 'path';
import { MemexConfigModule } from './config';
import { BrainContext } from './context/brain/brain.context';

/**
 * 应用根模块
 * 组织所有 Bounded Context
 */
@Module({
  imports: [
    MemexConfigModule,
    BrainContext,
    // 静态文件服务 - 服务前端构建产物
    ServeStaticModule.forRoot({
      rootPath: join(__dirname, '..', 'web', 'dist'),
      exclude: ['/api/(.*)'],
    }),
  ],
})
export class AppModule {}
