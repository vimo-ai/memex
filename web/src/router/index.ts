import { createRouter, createWebHistory, type RouteRecordRaw } from 'vue-router'

const routes: RouteRecordRaw[] = [
  // 根路径重定向到搜索页
  {
    path: '/',
    redirect: '/search',
  },
  // 搜索页 - 支持 query params: ?q=xxx&mode=hybrid&projectId=1&startDate=xxx&endDate=xxx
  {
    path: '/search',
    name: 'search',
    component: () => import('@/views/SearchView.vue'),
    meta: { title: '搜索' },
  },
  // 项目列表
  {
    path: '/projects',
    name: 'projects',
    component: () => import('@/views/ProjectsView.vue'),
    meta: { title: '项目列表' },
  },
  // 项目详情
  {
    path: '/projects/:projectId',
    name: 'project-detail',
    component: () => import('@/views/ProjectDetailView.vue'),
    meta: { title: '项目详情' },
  },
  // 会话详情 - 保留项目上下文
  {
    path: '/projects/:projectId/sessions/:sessionId',
    name: 'session-detail',
    component: () => import('@/views/SessionDetailView.vue'),
    meta: { title: '会话详情' },
  },
  // 兼容旧路由：独立会话访问（从搜索结果跳转）
  {
    path: '/sessions/:sessionId',
    name: 'session-standalone',
    component: () => import('@/views/SessionDetailView.vue'),
    meta: { title: '会话详情' },
  },
  // RAG 问答
  {
    path: '/ask',
    name: 'ask',
    component: () => import('@/views/AskView.vue'),
    meta: { title: 'Ask AI' },
  },
  // 404
  {
    path: '/:pathMatch(.*)*',
    name: 'not-found',
    component: () => import('@/views/NotFoundView.vue'),
    meta: { title: '页面不存在' },
  },
]

const router = createRouter({
  history: createWebHistory(),
  routes,
})

// 路由标题
router.beforeEach((to, _from, next) => {
  const title = to.meta.title as string | undefined
  document.title = title ? `${title} - Memex` : 'Memex'
  next()
})

export default router
