import { Controller, Get } from '@nestjs/common';

/**
 * 健康检查控制器
 */
@Controller('health')
export class HealthController {
  @Get()
  check() {
    return { status: 'ok', timestamp: new Date().toISOString() };
  }
}
