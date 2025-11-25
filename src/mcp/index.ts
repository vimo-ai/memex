#!/usr/bin/env node

/**
 * Memex MCP Server
 *
 * Provides conversation history search capabilities for Claude Code
 */

import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import {
  ListToolsRequestSchema,
  CallToolRequestSchema,
  Tool,
} from '@modelcontextprotocol/sdk/types.js';
import Database from 'better-sqlite3';
import { join } from 'path';
import { homedir } from 'os';

// Database path
const DB_PATH = join(homedir(), 'memex-data', 'memex.db');

// Initialize database connection
const db = new Database(DB_PATH, { readonly: true });

/**
 * Tool definitions
 */
const TOOLS: Tool[] = [
  {
    name: 'search_history',
    description:
      'Search Claude Code conversation history. Supports full-text search (FTS), vector semantic search, or hybrid mode. Can filter by project using cwd.',
    inputSchema: {
      type: 'object',
      properties: {
        query: {
          type: 'string',
          description: 'Search keywords',
        },
        mode: {
          type: 'string',
          enum: ['fts', 'vector', 'hybrid'],
          description:
            'Search mode: fts (full-text), vector (semantic), hybrid (combined). Default: hybrid',
        },
        cwd: {
          type: 'string',
          description:
            'Current working directory for project matching. Returns all projects if no match found',
        },
        limit: {
          type: 'number',
          description: 'Maximum number of results. Default: 10',
        },
      },
      required: ['query'],
    },
  },
  {
    name: 'get_session',
    description:
      'Get session details and message list. Supports pagination and in-session search.',
    inputSchema: {
      type: 'object',
      properties: {
        sessionId: {
          type: 'string',
          description: 'Session ID (full UUID or prefix)',
        },
        offset: {
          type: 'number',
          description: 'Message offset. Default: 0',
        },
        limit: {
          type: 'number',
          description: 'Number of messages to return. Default: 10',
        },
        search: {
          type: 'string',
          description:
            'In-session search keyword. Auto-navigates to first matching message',
        },
      },
      required: ['sessionId'],
    },
  },
  {
    name: 'get_recent_sessions',
    description:
      'Get recent sessions list, sorted by update time descending. Can filter by project using cwd.',
    inputSchema: {
      type: 'object',
      properties: {
        cwd: {
          type: 'string',
          description:
            'Current working directory for project matching. Returns all projects if no match found',
        },
        limit: {
          type: 'number',
          description: 'Number of sessions to return. Default: 5',
        },
      },
    },
  },
  {
    name: 'list_projects',
    description: 'List all indexed projects with session counts.',
    inputSchema: {
      type: 'object',
      properties: {},
    },
  },
];

/**
 * Find project ID by cwd
 */
function findProjectByCwd(cwd?: string): number | undefined {
  if (!cwd) return undefined;

  const stmt = db.prepare('SELECT id FROM projects WHERE path = ?');
  const result = stmt.get(cwd) as { id: number } | undefined;
  return result?.id;
}

/**
 * Find session by ID prefix
 */
function findSessionByIdPrefix(sessionIdPrefix: string): string | null {
  const stmt = db.prepare(
    'SELECT id FROM sessions WHERE id LIKE ? LIMIT 1'
  );
  const result = stmt.get(`${sessionIdPrefix}%`) as { id: string } | undefined;
  return result?.id || null;
}

/**
 * search_history tool implementation
 */
function searchHistory(args: {
  query: string;
  mode?: 'fts' | 'vector' | 'hybrid';
  cwd?: string;
  limit?: number;
}) {
  const { query, mode = 'fts', cwd, limit = 10 } = args;

  // Vector search requires Ollama service
  if (mode === 'vector') {
    return {
      error: 'Vector search requires Ollama service. Please use fts mode instead.',
    };
  }

  const projectId = findProjectByCwd(cwd);

  // Build query
  const conditions = ['messages_fts MATCH ?'];
  const params: any[] = [query];

  if (projectId !== undefined) {
    conditions.push('s.project_id = ?');
    params.push(projectId);
  }

  const whereClause = conditions.join(' AND ');

  const stmt = db.prepare(`
    SELECT
      m.id as messageId,
      m.uuid,
      m.content,
      m.type as messageType,
      m.session_id as sessionId,
      m.timestamp,
      s.project_id as projectId,
      p.name as projectName,
      snippet(messages_fts, 0, '[', ']', '...', 64) as snippet,
      rank as score
    FROM messages_fts
    JOIN messages m ON messages_fts.rowid = m.id
    JOIN sessions s ON m.session_id = s.id
    JOIN projects p ON s.project_id = p.id
    WHERE ${whereClause}
    ORDER BY rank
    LIMIT ?
  `);

  const rows = stmt.all(...params, limit) as any[];

  // Calculate messageIndex (position in session)
  const results = rows.map((row) => {
    // Query message index in session
    const indexStmt = db.prepare(`
      SELECT COUNT(*) as messageIndex
      FROM messages
      WHERE session_id = ? AND id <= ?
    `);
    const indexResult = indexStmt.get(row.sessionId, row.messageId) as { messageIndex: number };

    return {
      sessionId: row.sessionId,
      messageIndex: indexResult.messageIndex - 1, // 0-based index
      content: row.content,
      type: row.messageType,
      timestamp: row.timestamp,
      projectName: row.projectName,
      score: row.score,
      snippet: row.snippet,
    };
  });

  return { results };
}

/**
 * get_session tool implementation
 */
function getSession(args: {
  sessionId: string;
  offset?: number;
  limit?: number;
  search?: string;
}) {
  const { sessionId: sessionIdInput, offset = 0, limit = 10, search } = args;

  // Support ID prefix matching
  const sessionId = findSessionByIdPrefix(sessionIdInput) || sessionIdInput;

  // Query session info
  const sessionStmt = db.prepare(`
    SELECT s.*, p.name as projectName
    FROM sessions s
    JOIN projects p ON s.project_id = p.id
    WHERE s.id = ?
  `);
  const session = sessionStmt.get(sessionId) as any;

  if (!session) {
    return { error: `Session not found: ${sessionIdInput}` };
  }

  let actualOffset = offset;
  let searchMatchIndex: number | undefined;

  // If search param provided, find the first matching message
  if (search) {
    const searchStmt = db.prepare(`
      SELECT m.id, (
        SELECT COUNT(*) FROM messages WHERE session_id = ? AND id <= m.id
      ) - 1 as messageIndex
      FROM messages_fts
      JOIN messages m ON messages_fts.rowid = m.id
      WHERE m.session_id = ? AND messages_fts MATCH ?
      ORDER BY rank
      LIMIT 1
    `);
    const searchResult = searchStmt.get(sessionId, sessionId, search) as
      | { messageIndex: number }
      | undefined;

    if (searchResult) {
      searchMatchIndex = searchResult.messageIndex;
      // Adjust offset to include matched message in results
      actualOffset = Math.max(0, searchMatchIndex - Math.floor(limit / 2));
    }
  }

  // Query message list
  const messagesStmt = db.prepare(`
    SELECT
      id,
      uuid,
      type,
      content,
      timestamp,
      (SELECT COUNT(*) FROM messages WHERE session_id = ? AND id <= messages.id) - 1 as messageIndex
    FROM messages
    WHERE session_id = ?
    ORDER BY id ASC
    LIMIT ? OFFSET ?
  `);
  const messages = messagesStmt.all(sessionId, sessionId, limit, actualOffset) as any[];

  // Mark matched messages
  const messagesWithMatch = messages.map((msg) => ({
    index: msg.messageIndex,
    type: msg.type,
    content: msg.content,
    timestamp: msg.timestamp,
    matched: searchMatchIndex !== undefined && msg.messageIndex === searchMatchIndex,
  }));

  return {
    sessionId: session.id,
    projectName: session.projectName,
    totalMessages: session.message_count,
    currentOffset: actualOffset,
    messages: messagesWithMatch,
  };
}

/**
 * get_recent_sessions tool implementation
 */
function getRecentSessions(args: { cwd?: string; limit?: number }) {
  const { cwd, limit = 5 } = args;

  const projectId = findProjectByCwd(cwd);

  let stmt: any;
  let params: any[];

  if (projectId !== undefined) {
    stmt = db.prepare(`
      SELECT
        s.id,
        s.message_count as messageCount,
        s.updated_at as updatedAt,
        p.name as projectName,
        (SELECT content FROM messages WHERE session_id = s.id AND type = 'user' ORDER BY id ASC LIMIT 1) as preview
      FROM sessions s
      JOIN projects p ON s.project_id = p.id
      WHERE s.project_id = ?
      ORDER BY s.updated_at DESC
      LIMIT ?
    `);
    params = [projectId, limit];
  } else {
    stmt = db.prepare(`
      SELECT
        s.id,
        s.message_count as messageCount,
        s.updated_at as updatedAt,
        p.name as projectName,
        (SELECT content FROM messages WHERE session_id = s.id AND type = 'user' ORDER BY id ASC LIMIT 1) as preview
      FROM sessions s
      JOIN projects p ON s.project_id = p.id
      ORDER BY s.updated_at DESC
      LIMIT ?
    `);
    params = [limit];
  }

  const rows = stmt.all(...params) as any[];

  const sessions = rows.map((row) => ({
    id: row.id,
    projectName: row.projectName,
    messageCount: row.messageCount,
    updatedAt: row.updatedAt,
    preview: row.preview ? row.preview.substring(0, 100) : '',
  }));

  return { sessions };
}

/**
 * list_projects tool implementation
 */
function listProjects() {
  const stmt = db.prepare(`
    SELECT
      p.id,
      p.name,
      p.path,
      COUNT(s.id) as sessionCount
    FROM projects p
    LEFT JOIN sessions s ON p.id = s.project_id
    GROUP BY p.id
    ORDER BY p.name
  `);

  const rows = stmt.all() as any[];

  const projects = rows.map((row) => ({
    id: row.id,
    name: row.name,
    path: row.path,
    sessionCount: row.sessionCount,
  }));

  return { projects };
}

/**
 * Main function
 */
async function main() {
  const server = new Server(
    {
      name: 'memex',
      version: '1.0.0',
    },
    {
      capabilities: {
        tools: {},
      },
    }
  );

  // Register tools list
  server.setRequestHandler(ListToolsRequestSchema, async () => ({
    tools: TOOLS,
  }));

  // Register tool call handler
  server.setRequestHandler(CallToolRequestSchema, async (request) => {
    try {
      const { name, arguments: args } = request.params;

      let result: any;

      switch (name) {
        case 'search_history':
          result = searchHistory(args as any);
          break;
        case 'get_session':
          result = getSession(args as any);
          break;
        case 'get_recent_sessions':
          result = getRecentSessions(args as any);
          break;
        case 'list_projects':
          result = listProjects();
          break;
        default:
          throw new Error(`Unknown tool: ${name}`);
      }

      return {
        content: [
          {
            type: 'text',
            text: JSON.stringify(result, null, 2),
          },
        ],
      };
    } catch (error: any) {
      return {
        content: [
          {
            type: 'text',
            text: JSON.stringify({ error: error.message }, null, 2),
          },
        ],
        isError: true,
      };
    }
  });

  // Connect stdio transport
  const transport = new StdioServerTransport();
  await server.connect(transport);

  // Graceful shutdown
  process.on('SIGINT', () => {
    db.close();
    process.exit(0);
  });

  process.on('SIGTERM', () => {
    db.close();
    process.exit(0);
  });
}

main().catch((error) => {
  console.error('MCP Server failed to start:', error);
  process.exit(1);
});
