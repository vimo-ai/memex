import { Module } from '@nestjs/common';
import { MemexConfigModule } from './config';
import { BrainContext } from './context/brain/brain.context';

/**
 * 应用根模块
 * 组织所有 Bounded Context
 */
@Module({
  imports: [MemexConfigModule, BrainContext],
})
export class AppModule {}
