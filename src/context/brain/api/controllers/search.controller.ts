import { Controller, Get, Query } from '@nestjs/common';
import { SearchService } from '../../application/services/search.service';
import { SearchQueryDto, SearchResponseDto, SearchResultItemDto } from '../dto/search.dto';

/**
 * 搜索控制器
 *
 * 提供全文搜索 HTTP API
 */
@Controller('search')
export class SearchController {
  constructor(private readonly searchService: SearchService) {}

  /**
   * 全文搜索
   *
   * GET /api/search?q=keyword&project_id=1&limit=20
   */
  @Get()
  search(@Query() query: SearchQueryDto): SearchResponseDto {
    const results = this.searchService.search({
      query: query.q,
      projectId: query.project_id,
      limit: query.limit,
    });

    return {
      query: query.q,
      total: results.length,
      results: results.map((r) => this.toResultItemDto(r)),
    };
  }

  /**
   * 搜索结果转 DTO
   */
  private toResultItemDto(result: {
    message: {
      id?: number;
      uuid: string;
      sessionId: string;
      type: string;
      content: string;
      timestamp?: Date;
    };
    snippet?: string;
    rank?: number;
  }): SearchResultItemDto {
    return {
      messageId: result.message.id!,
      messageUuid: result.message.uuid,
      sessionId: result.message.sessionId,
      type: result.message.type,
      content: result.message.content,
      snippet: result.snippet,
      rank: result.rank,
      timestamp: result.message.timestamp?.toISOString(),
    };
  }
}
