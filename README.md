[English](README.md) | [中文](/.github/README_zh-CN.md)

# Memex

A session history management system for Claude Code. Never lose your conversations again.

## Why Memex?

Claude Code's local conversation data expires after 30 days, causing:
- Loss of important technical decision records
- Difficulty searching historical conversations
- Knowledge cannot be accumulated and reused

Memex solves these problems:
- ✅ Automatic backup of all Claude Code sessions
- ✅ Powerful full-text and semantic search
- ✅ MCP protocol support for searching history directly in Claude
- ✅ Web UI for browsing and managing sessions

## Features

### Data Collection & Backup
- Automatically scans all sessions under `~/.claude/projects/`
- Parses JSONL format conversation content
- Stores in SQLite database
- Supports daily incremental backups

### Search Capabilities
- **Full-text Search**: Fast keyword search based on SQLite FTS5
- **Semantic Search**: Semantic understanding using Ollama + LanceDB
- **Hybrid Retrieval**: RRF fusion ranking combining keyword and semantic relevance
- **Advanced Filtering**: Filter by project, time range, Session ID prefix

### MCP Integration
Search historical conversations in Claude Code via MCP protocol:
- `search_history` - Search historical conversations
- `get_session` - Get session details (supports pagination and in-session search)
- `get_recent_sessions` - Get recent sessions
- `list_projects` - List all projects

### Web UI
- Cyberpunk-style interface
- Project list and session browsing
- Quick lookup by Session ID prefix
- Supports full-text/semantic/hybrid search

## Tech Stack

- **Backend**: NestJS (DDD architecture)
- **Database**: SQLite + FTS5 (full-text search)
- **Vector Store**: LanceDB
- **Embedding**: Ollama (bge-m3 model)
- **Frontend**: Vue 3
- **Communication**: HTTP + JSON-RPC (MCP)

## Installation

### Prerequisites

1. Node.js >= 18
2. pnpm (recommended)
3. Ollama (for semantic search, optional)

```bash
# Install Ollama
curl -fsSL https://ollama.com/install.sh | sh

# Pull embedding model
ollama pull bge-m3
```

### Install Project

```bash
# Clone project
git clone <repository-url>
cd memex

# Install dependencies
pnpm install

# Web UI dependencies
cd web
pnpm install
cd ..
```

## Configuration

Copy and edit the configuration file:

```bash
cp .env.example .env
```

Main configuration options:

```bash
# Server port
PORT=10013

# Data storage directory
MEMEX_DATA_DIR=~/memex-data

# Backup directory
MEMEX_BACKUP_DIR=~/memex-data/backups

# Claude Code data source path
CLAUDE_PROJECTS_PATH=~/.claude/projects

# Ollama API address
OLLAMA_API=http://localhost:11434/api

# Embedding model
EMBEDDING_MODEL=bge-m3
```

## Running

### Development Mode

```bash
# Start backend
pnpm dev

# Start frontend (new terminal)
cd web
pnpm dev
```

### Production Mode

```bash
# Build backend
pnpm build

# Build frontend
cd web
pnpm build
cd ..

# Start service
pnpm start:prod
```

## MCP Configuration

Memex provides MCP service via HTTP protocol with simple configuration.

### Option 1: mcp-router Configuration (Recommended)

Edit mcp-router configuration file:

```json
{
  "mcpServers": {
    "memex": {
      "type": "http",
      "url": "http://127.0.0.1:10013/api/mcp"
    }
  }
}
```

### Option 2: Claude Code Direct Configuration

Add to Claude Code's MCP settings:

```json
{
  "mcpServers": {
    "memex": {
      "type": "http",
      "url": "http://127.0.0.1:10013/api/mcp"
    }
  }
}
```

### Verify MCP Connection

After starting Claude Code, verify with:

```
Search for my recent discussions about DDD architecture
```

If MCP is configured correctly, Claude will call the `memex/search_history` tool.

## API Endpoints

### Project Management
- `GET /api/projects` - Get all projects list
- `GET /api/projects/:id` - Get project details (including session list)

### Session Management
- `GET /api/sessions/:id` - Get session details (full conversation content)
- `GET /api/sessions/search?idPrefix=xxx` - Search by Session ID prefix

### Search
- `GET /api/search?q=xxx&projectId=yyy` - Full-text search
  - Query parameters:
    - `q`: Search keywords
    - `projectId`: Project filter (optional)
    - `startDate`: Start date (optional)
    - `endDate`: End date (optional)
    - `limit`: Result limit, default 20

- `GET /api/search/semantic?q=xxx&mode=hybrid` - Semantic search
  - Query parameters:
    - `q`: Search content
    - `mode`: Search mode
      - `semantic`: Pure semantic search
      - `hybrid`: Hybrid search (keyword + semantic)
    - `projectId`: Project filter (optional)
    - `limit`: Result limit, default 10

### MCP
- `POST /api/mcp` - MCP JSON-RPC endpoint
- `GET /api/mcp/info` - Get MCP tools information

## Usage Examples

### Web UI

Visit `http://localhost:10013` to use the web interface.

Main features:
- Browse all projects and sessions
- Quick lookup by Session ID prefix
- Full-text search conversation content
- Semantic search related discussions
- Filter by project and time

### Command Line Search

```bash
# Full-text search
curl "http://localhost:10013/api/search?q=authentication"

# Semantic search
curl "http://localhost:10013/api/search/semantic?q=how+to+design+domain+models&mode=hybrid"

# Search by project
curl "http://localhost:10013/api/search?q=bug&projectId=xxx"
```

### MCP Usage

Ask directly in Claude Code:

```
Search for previous discussions about NestJS dependency injection

Find sessions from the last week and see what we worked on

Get the full session content about database design
```

## Data Directory Structure

```
~/memex-data/
├── memex.db              # SQLite database
├── vectors/              # LanceDB vector storage
│   └── messages/
└── backups/              # Backup files
    └── memex-2025-01-15.db
```

## FAQ

### Q: How to trigger initial data import?

A: The service automatically scans `~/.claude/projects/` and imports all sessions on startup. You can also trigger manually via API:

```bash
curl -X POST http://localhost:10013/api/backup
```

### Q: Semantic search not working?

A: Ensure:
1. Ollama service is running: `ollama serve`
2. Model is downloaded: `ollama pull bge-m3`
3. `OLLAMA_API` is configured correctly in `.env`

### Q: How to clear and rebuild index?

A: Delete the data directory and restart the service:

```bash
rm -rf ~/memex-data
pnpm start
```

### Q: MCP connection failed?

A: Check:
1. Memex service is running at `http://localhost:10013`
2. MCP configuration path is correct
3. Node.js version is >= 18

## Development

### Project Structure

```
memex/
├── src/
│   ├── context/                 # DDD contexts
│   │   └── brain/              # Core context
│   │       ├── api/            # API layer
│   │       ├── application/    # Application services
│   │       ├── domain/         # Domain models
│   │       └── infrastructure/ # Infrastructure
│   └── main.ts                 # Application entry
├── web/                        # Vue frontend
└── DESIGN.md                   # Architecture design document
```

### Running Tests

```bash
pnpm test
```

## Roadmap

- [x] Phase 0: Data collection and backup
- [x] Phase 1: SQLite + FTS5 full-text search
- [x] Phase 2: Semantic search (Ollama + LanceDB)
- [x] Phase 3: MCP integration
- [x] Web UI
- [ ] Phase 4: RAG Q&A
- [ ] Phase 5: Knowledge distillation

## License

MIT

## Acknowledgments

Thanks to Claude Code for providing such an excellent development experience.
