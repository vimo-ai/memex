/**
 * 项目实体
 *
 * 代表一个 Claude Code 项目的元数据
 */
export class ProjectEntity {
  /** 数据库 ID */
  id?: number;

  /** 项目真实路径 */
  path: string;

  /** 项目名称（从路径提取） */
  name: string;

  /** 编码的目录名（Claude Code 存储使用的目录名） */
  encodedDirName: string;

  /** 创建时间 */
  createdAt?: Date;

  /** 更新时间 */
  updatedAt?: Date;

  constructor(props: {
    id?: number;
    path: string;
    name: string;
    encodedDirName: string;
    createdAt?: Date;
    updatedAt?: Date;
  }) {
    this.id = props.id;
    this.path = props.path;
    this.name = props.name;
    this.encodedDirName = props.encodedDirName;
    this.createdAt = props.createdAt;
    this.updatedAt = props.updatedAt;
  }

  /**
   * 从项目路径创建实体
   * @param path 项目真实路径
   * @param encodedDirName Claude Code 编码的目录名
   */
  static fromPath(path: string, encodedDirName: string): ProjectEntity {
    // 从路径提取项目名称（最后一个目录名）
    const name = path.split('/').filter(Boolean).pop() || path;

    return new ProjectEntity({
      path,
      name,
      encodedDirName,
    });
  }
}
