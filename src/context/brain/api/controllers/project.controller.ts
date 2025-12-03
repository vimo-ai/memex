import { Controller, Get, Param, ParseIntPipe, NotFoundException } from '@nestjs/common';
import { Inject } from '@nestjs/common';
import {
  IProjectRepository,
  PROJECT_REPOSITORY,
} from '../../domain/repositories/project.repository.interface';
import {
  ISessionRepository,
  SESSION_REPOSITORY,
} from '../../domain/repositories/session.repository.interface';
import {
  ProjectResponseDto,
  ProjectListResponseDto,
  ProjectDetailResponseDto,
} from '../dto/project.dto';
import { SessionResponseDto, SessionListResponseDto } from '../dto/session.dto';
import { ProjectEntity } from '../../domain/entities/project.entity';
import { SessionEntity } from '../../domain/entities/session.entity';

/**
 * 项目控制器
 *
 * 提供项目相关的 HTTP API
 */
@Controller('projects')
export class ProjectController {
  constructor(
    @Inject(PROJECT_REPOSITORY)
    private readonly projectRepository: IProjectRepository,
    @Inject(SESSION_REPOSITORY)
    private readonly sessionRepository: ISessionRepository,
  ) {}

  /**
   * 获取项目列表
   *
   * GET /api/projects
   */
  @Get()
  getProjects(): ProjectListResponseDto {
    const projects = this.projectRepository.findAll();

    return {
      total: projects.length,
      projects: projects.map((p) => this.toProjectDto(p)),
    };
  }

  /**
   * 获取单个项目详情
   *
   * GET /api/projects/:id
   */
  @Get(':id')
  getProject(@Param('id', ParseIntPipe) id: number): ProjectDetailResponseDto {
    const project = this.projectRepository.findById(id);

    if (!project) {
      throw new NotFoundException(`项目不存在: ${id}`);
    }

    // 获取会话统计
    const sessions = this.sessionRepository.findSessionsByProjectId(id);
    const messageCount = sessions.reduce((sum, s) => sum + s.messageCount, 0);

    return {
      ...this.toProjectDto(project),
      sessionCount: sessions.length,
      messageCount,
    };
  }

  /**
   * 获取项目的会话列表
   *
   * GET /api/projects/:id/sessions
   */
  @Get(':id/sessions')
  getProjectSessions(@Param('id', ParseIntPipe) id: number): SessionListResponseDto {
    const project = this.projectRepository.findById(id);

    if (!project) {
      throw new NotFoundException(`项目不存在: ${id}`);
    }

    const sessions = this.sessionRepository.findSessionsByProjectId(id);

    return {
      total: sessions.length,
      sessions: sessions.map((s) => this.toSessionDto(s)),
    };
  }

  /**
   * 项目实体转 DTO
   */
  private toProjectDto(project: ProjectEntity): ProjectResponseDto {
    return {
      id: project.id!,
      name: project.name,
      path: project.path,
      encodedDirName: project.encodedDirName,
      source: project.source,
      createdAt: project.createdAt?.toISOString() ?? new Date().toISOString(),
      updatedAt: project.updatedAt?.toISOString() ?? new Date().toISOString(),
    };
  }

  /**
   * 会话实体转 DTO
   */
  private toSessionDto(session: SessionEntity): SessionResponseDto {
    return {
      id: session.id,
      projectId: session.projectId,
      status: session.status,
      source: session.source,
      channel: session.channel,
      cwd: session.cwd,
      model: session.model,
      meta: session.meta,
      messageCount: session.messageCount,
      createdAt: session.createdAt?.toISOString() ?? new Date().toISOString(),
      updatedAt: session.updatedAt?.toISOString() ?? new Date().toISOString(),
    };
  }
}
