/**
 * 项目响应 DTO
 */
export class ProjectResponseDto {
  /** 项目 ID */
  id!: number;

  /** 项目名称 */
  name!: string;

  /** 项目路径 */
  path!: string;

  /** 编码目录名 */
  encodedDirName!: string;

  /** 创建时间 */
  createdAt!: string;

  /** 更新时间 */
  updatedAt!: string;

  /** 会话数量（可选，详情接口返回） */
  sessionCount?: number;
}

/**
 * 项目列表响应 DTO
 */
export class ProjectListResponseDto {
  /** 项目总数 */
  total!: number;

  /** 项目列表 */
  projects!: ProjectResponseDto[];
}

/**
 * 项目详情响应 DTO（包含统计信息）
 */
export class ProjectDetailResponseDto extends ProjectResponseDto {
  /** 会话数量 */
  sessionCount!: number;

  /** 消息总数 */
  messageCount!: number;
}
