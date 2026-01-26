// API 基础路径
const BASE_URL = '/api'

// 通用请求方法
async function request<T>(url: string, options?: RequestInit): Promise<T> {
  const response = await fetch(`${BASE_URL}${url}`, {
    headers: {
      'Content-Type': 'application/json',
    },
    ...options,
  })

  if (!response.ok) {
    throw new Error(`HTTP error! status: ${response.status}`)
  }

  return response.json()
}

// 项目相关类型
export interface Project {
  id: number
  path: string
  source?: string
  sessionCount: number
  createdAt: string
  updatedAt: string
}

// 会话相关类型
export interface Session {
  id: string // UUID 字符串
  projectId: number
  status: string
  source?: string
  channel?: string
  cwd?: string
  model?: string
  messageCount: number
  createdAt: string
  updatedAt: string
}

// 消息相关类型
export interface Message {
  id: number
  uuid: string
  sessionId: string
  type: 'user' | 'assistant' | 'tool'
  source?: string
  channel?: string
  model?: string
  toolCallId?: string
  toolName?: string
  toolArgs?: string
  raw?: string
  content: string
  timestamp: string
  createdAt: string
}

// 搜索结果类型
export interface SearchResult {
  messageId: number
  sessionId: string
  messageUuid: string
  type: 'user' | 'assistant' | 'tool'
  source?: string
  channel?: string
  content: string
  snippet: string
  rank: number
  timestamp: string
}

// 搜索响应类型
export interface SearchResponse {
  query: string
  total: number
  results: SearchResult[]
}

// 统计信息类型
export interface Stats {
  projectCount: number
  sessionCount: number
  messageCount: number
  lastCollectedAt: string | null
}

// 分页响应类型
export interface PaginatedResponse<T> {
  data: T[]
  total: number
  limit: number
  offset: number
}

// API 方法

// 项目列表响应类型
interface ProjectsResponse {
  total: number
  projects: Project[]
}

// 会话列表响应类型
interface SessionsResponse {
  total: number
  sessions: Session[]
}

// 消息列表响应类型
interface MessagesResponse {
  total: number
  messages: Message[]
}

/**
 * 获取项目列表
 */
export async function getProjects(limit = 100): Promise<Project[]> {
  const response = await request<ProjectsResponse>(`/projects?limit=${limit}`)
  return response.projects
}

/**
 * 获取单个项目
 */
export async function getProject(id: number): Promise<Project> {
  return request<Project>(`/projects/${id}`)
}

/**
 * 获取项目下的会话列表
 */
export async function getProjectSessions(
  projectId: number,
  limit = 50
): Promise<Session[]> {
  const response = await request<SessionsResponse>(`/projects/${projectId}/sessions?limit=${limit}`)
  return response.sessions
}

/**
 * 获取单个会话
 */
export async function getSession(id: string): Promise<Session> {
  return request<Session>(`/sessions/${id}`)
}

/**
 * 获取会话的消息列表
 */
export async function getSessionMessages(
  sessionId: string,
  limit = 100,
  offset = 0
): Promise<PaginatedResponse<Message>> {
  const response = await request<MessagesResponse>(
    `/sessions/${sessionId}/messages?limit=${limit}&offset=${offset}`
  )
  return {
    data: response.messages,
    total: response.total,
    limit,
    offset,
  }
}

/**
 * 搜索消息
 */
export async function search(
  query: string,
  projectId?: number,
  limit = 20
): Promise<SearchResult[]> {
  const params = new URLSearchParams({
    q: query,
    limit: String(limit),
  })

  if (projectId) {
    params.append('projectId', String(projectId))
  }

  const response = await request<SearchResponse>(`/search?${params.toString()}`)
  return response.results
}

/**
 * 获取统计信息
 */
export async function getStats(): Promise<Stats> {
  return request<Stats>('/admin/stats')
}

// ========== 语义搜索 API ==========

/** 语义搜索模式 */
export type SemanticSearchMode = 'fts' | 'vector' | 'hybrid'

/** 语义搜索结果类型 */
export interface SemanticSearchResult {
  messageId: number
  uuid: string
  content: string
  messageType: 'user' | 'assistant' | 'tool'
  sessionId: string
  projectId: number
  projectName: string
  source?: string
  channel?: string
  model?: string
  timestamp: string
  snippet?: string
  score: number
  sources: {
    fts: boolean
    vector: boolean
  }
  ftsRank?: number
  vectorSimilarity?: number
}

/** 语义搜索响应 */
export interface SemanticSearchResponse {
  results: SemanticSearchResult[]
  total: number
  mode: SemanticSearchMode
}

/** 索引状态 */
export interface EmbeddingStats {
  pending: number
  failed: number
  indexed: number
  embeddingAvailable: boolean
  embeddingModel: string
  isRunning: boolean
}

/**
 * 语义搜索
 */
export async function semanticSearch(
  query: string,
  options?: {
    limit?: number
    projectId?: number
    mode?: SemanticSearchMode
    startDate?: string
    endDate?: string
  }
): Promise<SemanticSearchResult[]> {
  const params = new URLSearchParams({ q: query })

  if (options?.limit) params.append('limit', String(options.limit))
  if (options?.projectId) params.append('projectId', String(options.projectId))
  if (options?.mode) params.append('mode', options.mode)
  if (options?.startDate) params.append('startDate', options.startDate)
  if (options?.endDate) params.append('endDate', options.endDate)

  const response = await request<SemanticSearchResponse>(`/search/semantic?${params.toString()}`)
  return response.results
}

/**
 * 获取索引状态
 */
export async function getEmbeddingStats(): Promise<EmbeddingStats> {
  return request<EmbeddingStats>('/embedding/stats')
}

/**
 * 触发数据采集
 */
export async function triggerCollect(): Promise<{ success: boolean; message: string }> {
  return request('/admin/collect', { method: 'POST' })
}

/**
 * 根据 ID 前缀搜索会话
 */
export async function searchSessionsByIdPrefix(
  idPrefix: string,
  limit = 20
): Promise<Session[]> {
  const params = new URLSearchParams({
    idPrefix,
    limit: String(limit),
  })

  interface SearchResponse {
    query: string
    total: number
    sessions: Session[]
  }

  const response = await request<SearchResponse>(`/sessions/search?${params.toString()}`)
  return response.sessions
}

// ========== RAG 问答 API ==========

export interface RagRequest {
  question: string
  cwd?: string
  contextWindow?: number
  maxSources?: number
}

export interface RagResponse {
  answer: string
  sources: Array<{
    sessionId: string
    projectName: string
    messageIndex: number
    snippet: string
    score: number
  }>
  model: string
  tokensUsed?: number
}

/**
 * RAG 问答
 */
export async function ask(data: RagRequest): Promise<RagResponse> {
  return request<RagResponse>('/ask', {
    method: 'POST',
    body: JSON.stringify(data),
  })
}
