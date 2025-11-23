import { IsOptional, IsInt, Min, Max } from 'class-validator';
import { Type } from 'class-transformer';

/**
 * 通用分页请求 DTO
 */
export class PaginationQueryDto {
  /** 页码（从 1 开始） */
  @IsOptional()
  @Type(() => Number)
  @IsInt()
  @Min(1)
  page?: number = 1;

  /** 每页数量 */
  @IsOptional()
  @Type(() => Number)
  @IsInt()
  @Min(1)
  @Max(100)
  limit?: number = 20;
}

/**
 * 分页响应元数据
 */
export class PaginationMeta {
  /** 当前页码 */
  page!: number;

  /** 每页数量 */
  limit!: number;

  /** 总数量 */
  total!: number;

  /** 总页数 */
  totalPages!: number;
}

/**
 * 分页响应 DTO
 */
export class PaginatedResponseDto<T> {
  /** 数据列表 */
  data!: T[];

  /** 分页元数据 */
  pagination!: PaginationMeta;

  /**
   * 创建分页响应
   */
  static create<T>(data: T[], total: number, page: number, limit: number): PaginatedResponseDto<T> {
    const response = new PaginatedResponseDto<T>();
    response.data = data;
    response.pagination = {
      page,
      limit,
      total,
      totalPages: Math.ceil(total / limit),
    };
    return response;
  }
}
