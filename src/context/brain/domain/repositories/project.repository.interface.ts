import { ProjectEntity } from '../entities/project.entity';

/**
 * 项目仓储接口注入 Token
 */
export const PROJECT_REPOSITORY = Symbol('PROJECT_REPOSITORY');

/**
 * 项目仓储接口
 *
 * 定义项目数据访问的契约
 */
export interface IProjectRepository {
  /**
   * 保存项目
   * - 如果项目不存在则创建
   * - 如果已存在（path 相同）则更新
   * @returns 保存后的项目（包含 ID）
   */
  save(project: ProjectEntity): ProjectEntity;

  /**
   * 根据 ID 查找项目
   */
  findById(id: number): ProjectEntity | null;

  /**
   * 根据路径查找项目
   */
  findByPath(path: string): ProjectEntity | null;

  /**
   * 根据编码目录名查找项目
   */
  findByEncodedDirName(encodedDirName: string): ProjectEntity | null;

  /**
   * 获取所有项目
   */
  findAll(): ProjectEntity[];

  /**
   * 删除项目
   */
  delete(id: number): boolean;

  /**
   * 统计项目数量
   */
  count(): number;
}
