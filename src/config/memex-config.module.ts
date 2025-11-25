import { Module, Global } from '@nestjs/common';
import { ConfigModule } from '@nestjs/config';
import { MemexConfigService } from './memex-config.service';

/**
 * Memex 配置模块
 * 全局模块，在整个应用中可用
 */
@Global()
@Module({
  imports: [
    ConfigModule.forRoot({
      isGlobal: true,
      envFilePath: ['.env.local', '.env'],
      cache: true,
    }),
  ],
  providers: [MemexConfigService],
  exports: [MemexConfigService],
})
export class MemexConfigModule {}
