import { Module } from '@nestjs/common';
import { BrainContext } from './context/brain/brain.context';

/**
 * 应用根模块
 * 组织所有 Bounded Context
 */
@Module({
  imports: [BrainContext],
})
export class AppModule {}
