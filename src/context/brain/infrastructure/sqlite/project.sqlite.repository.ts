import { Inject, Injectable } from '@nestjs/common';
import { SQLITE_DB, SqliteDatabase } from './sqlite.provider';
import { IProjectRepository } from '../../domain/repositories/project.repository.interface';
import { ProjectEntity } from '../../domain/entities/project.entity';

/**
 * 项目表数据行类型
 */
interface ProjectRow {
  id: number;
  path: string;
  name: string;
  encoded_dir_name: string | null;
  source: string | null;
  created_at: string;
  updated_at: string;
}

/**
 * 项目仓储 SQLite 实现
 */
@Injectable()
export class ProjectSqliteRepository implements IProjectRepository {
  constructor(@Inject(SQLITE_DB) private readonly db: SqliteDatabase) {}

  /**
   * 保存项目（UPSERT）
   * 根据 path + source 组合唯一约束进行插入或更新
   */
  save(project: ProjectEntity): ProjectEntity {
    const stmt = this.db.prepare(`
      INSERT INTO projects (path, name, encoded_dir_name, source, updated_at)
      VALUES (@path, @name, @encodedDirName, @source, CURRENT_TIMESTAMP)
      ON CONFLICT(path, source) DO UPDATE SET
        name = excluded.name,
        encoded_dir_name = excluded.encoded_dir_name,
        updated_at = CURRENT_TIMESTAMP
      RETURNING *
    `);

    const row = stmt.get({
      path: project.path,
      name: project.name,
      encodedDirName: project.encodedDirName ?? null,
      source: project.source ?? 'claude',
    }) as ProjectRow;

    return this.rowToEntity(row);
  }

  /**
   * 根据 ID 查找项目
   */
  findById(id: number): ProjectEntity | null {
    const stmt = this.db.prepare('SELECT * FROM projects WHERE id = ?');
    const row = stmt.get(id) as ProjectRow | undefined;
    return row ? this.rowToEntity(row) : null;
  }

  /**
   * 根据路径查找项目
   */
  findByPath(path: string): ProjectEntity | null {
    const stmt = this.db.prepare('SELECT * FROM projects WHERE path = ?');
    const row = stmt.get(path) as ProjectRow | undefined;
    return row ? this.rowToEntity(row) : null;
  }

  /**
   * 根据编码目录名查找项目
   */
  findByEncodedDirName(encodedDirName: string): ProjectEntity | null {
    const stmt = this.db.prepare(
      'SELECT * FROM projects WHERE encoded_dir_name = ?',
    );
    const row = stmt.get(encodedDirName) as ProjectRow | undefined;
    return row ? this.rowToEntity(row) : null;
  }

  /**
   * 获取所有项目
   * 通过 JOIN sessions 表获取每个项目的最新活动时间
   */
  findAll(): ProjectEntity[] {
    const stmt = this.db.prepare(`
      SELECT
        p.*,
        COALESCE(MAX(s.updated_at), p.updated_at) as latest_activity
      FROM projects p
      LEFT JOIN sessions s ON s.project_id = p.id
      GROUP BY p.id
      ORDER BY latest_activity DESC
    `);
    const rows = stmt.all() as (ProjectRow & { latest_activity: string })[];
    return rows.map((row) => this.rowToEntity({
      ...row,
      updated_at: row.latest_activity,
    }));
  }

  /**
   * 删除项目
   */
  delete(id: number): boolean {
    const stmt = this.db.prepare('DELETE FROM projects WHERE id = ?');
    const result = stmt.run(id);
    return result.changes > 0;
  }

  /**
   * 统计项目数量
   */
  count(): number {
    const stmt = this.db.prepare('SELECT COUNT(*) as count FROM projects');
    const result = stmt.get() as { count: number };
    return result.count;
  }

  /**
   * 数据行转换为实体
   */
  private rowToEntity(row: ProjectRow): ProjectEntity {
    return new ProjectEntity({
      id: row.id,
      path: row.path,
      name: row.name,
      encodedDirName: row.encoded_dir_name ?? undefined,
      source: row.source ?? undefined,
      createdAt: new Date(row.created_at),
      updatedAt: new Date(row.updated_at),
    });
  }
}
