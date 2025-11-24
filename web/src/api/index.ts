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
  sessionCount: number
  createdAt: string
  updatedAt: string
}

// 会话相关类型
export interface Session {
  id: string // UUID 字符串
  projectId: number
  status: string
  messageCount: number
  createdAt: string
  updatedAt: string
}

// 消息相关类型
export interface Message {
  id: number
  uuid: string
  sessionId: string
  type: 'user' | 'assistant'
  content: string
  timestamp: string
  createdAt: string
}

// 搜索结果类型
export interface SearchResult {
  messageId: number
  sessionId: string
  messageUuid: string
  type: 'user' | 'assistant'
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

/**
 * 触发数据采集
 */
export async function triggerCollect(): Promise<{ success: boolean; message: string }> {
  return request('/admin/collect', { method: 'POST' })
}
