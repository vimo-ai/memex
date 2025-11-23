import { IsString, IsOptional, IsInt, Min, Max } from 'class-validator';
import { Transform, Type } from 'class-transformer';

/**
 * 搜索请求 DTO
 */
export class SearchQueryDto {
  /** 搜索关键词 */
  @IsString()
  q!: string;

  /** 项目 ID（可选，限定搜索范围） */
  @IsOptional()
  @Type(() => Number)
  @IsInt()
  @Min(1)
  project_id?: number;

  /** 返回数量限制 */
  @IsOptional()
  @Type(() => Number)
  @IsInt()
  @Min(1)
  @Max(100)
  limit?: number = 20;
}

/**
 * 搜索结果项 DTO
 */
export class SearchResultItemDto {
  /** 消息 ID */
  messageId!: number;

  /** 消息 UUID */
  messageUuid!: string;

  /** 会话 ID */
  sessionId!: string;

  /** 消息类型 */
  type!: string;

  /** 消息内容 */
  content!: string;

  /** 匹配的高亮片段 */
  snippet?: string;

  /** 相关性得分 */
  rank?: number;

  /** 消息时间戳 */
  timestamp?: string;
}

/**
 * 搜索响应 DTO
 */
export class SearchResponseDto {
  /** 搜索关键词 */
  query!: string;

  /** 结果总数 */
  total!: number;

  /** 搜索结果列表 */
  results!: SearchResultItemDto[];
}
