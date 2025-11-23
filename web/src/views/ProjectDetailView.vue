<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { getProject, getProjectSessions, type Project, type Session } from '@/api'

const route = useRoute()
const router = useRouter()

// 项目信息
const project = ref<Project | null>(null)
// 会话列表
const sessions = ref<Session[]>([])
// 加载状态
const loading = ref(true)
// 错误信息
const error = ref<string | null>(null)

// 获取项目 ID
const projectId = Number(route.params.id)

// 加载数据
async function loadData() {
  loading.value = true
  error.value = null

  try {
    // 并行加载项目信息和会话列表
    const [projectData, sessionsData] = await Promise.all([
      getProject(projectId),
      getProjectSessions(projectId),
    ])
    project.value = projectData
    sessions.value = sessionsData
  } catch (e) {
    console.error('加载数据失败:', e)
    error.value = '加载失败，请稍后重试'
  } finally {
    loading.value = false
  }
}

// 跳转到会话详情
function goToSession(session: Session) {
  router.push(`/sessions/${session.id}`)
}

// 返回项目列表
function goBack() {
  router.push('/projects')
}

// 从路径中提取项目名称
function getProjectName(path: string): string {
  const parts = path.split('/')
  return parts[parts.length - 1] || path
}

// 格式化时间
function formatTime(timestamp: string): string {
  const date = new Date(timestamp)
  return date.toLocaleString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  })
}

// 格式化 UUID（只显示前 8 位）
function shortUuid(uuid: string): string {
  return uuid.slice(0, 8)
}

onMounted(() => {
  loadData()
})
</script>

<template>
  <div>
    <!-- 加载状态 -->
    <div v-if="loading" class="flex justify-center py-12">
      <div class="i-carbon-circle-dash animate-spin text-3xl text-blue-400" />
    </div>

    <!-- 错误提示 -->
    <div v-else-if="error" class="text-center py-12">
      <div class="i-carbon-warning text-4xl text-red-400 mb-4 mx-auto" />
      <p class="text-gray-400">{{ error }}</p>
      <button
        class="mt-4 px-4 py-2 bg-blue-600 hover:bg-blue-500 rounded-lg transition-colors"
        @click="loadData"
      >
        重试
      </button>
    </div>

    <template v-else-if="project">
      <!-- 面包屑导航 -->
      <div class="flex items-center gap-2 text-sm text-gray-500 mb-4">
        <button class="hover:text-gray-300 transition-colors" @click="goBack">项目列表</button>
        <span>/</span>
        <span class="text-gray-300">{{ getProjectName(project.path) }}</span>
      </div>

      <!-- 项目信息 -->
      <div class="bg-gray-800 rounded-lg border border-gray-700 p-4 mb-6">
        <div class="flex items-center gap-3 mb-2">
          <div class="i-carbon-folder text-2xl text-blue-400" />
          <h1 class="text-xl font-bold">{{ getProjectName(project.path) }}</h1>
        </div>
        <p class="text-gray-500 text-sm" :title="project.path">{{ project.path }}</p>
      </div>

      <!-- 会话列表标题 -->
      <div class="flex items-center justify-between mb-4">
        <h2 class="text-lg font-semibold">会话列表</h2>
        <span class="text-gray-500 text-sm">共 {{ sessions.length }} 个会话</span>
      </div>

      <!-- 空状态 -->
      <div v-if="sessions.length === 0" class="text-center py-12">
        <div class="i-carbon-chat text-4xl text-gray-500 mb-4 mx-auto" />
        <p class="text-gray-400">暂无会话数据</p>
      </div>

      <!-- 会话列表 -->
      <div v-else class="space-y-3">
        <div
          v-for="session in sessions"
          :key="session.id"
          class="p-4 bg-gray-800 rounded-lg border border-gray-700 hover:border-gray-600 cursor-pointer transition-all"
          @click="goToSession(session)"
        >
          <div class="flex items-center justify-between">
            <!-- 会话信息 -->
            <div class="flex items-center gap-3">
              <div class="i-carbon-chat text-lg text-green-400" />
              <div>
                <span class="font-mono text-sm text-gray-400">{{ shortUuid(session.uuid) }}...</span>
              </div>
            </div>

            <!-- 统计和时间 -->
            <div class="flex items-center gap-4 text-sm text-gray-400">
              <span class="flex items-center gap-1">
                <div class="i-carbon-document" />
                {{ session.messageCount }} 条消息
              </span>
              <span>{{ formatTime(session.updatedAt) }}</span>
              <div class="i-carbon-chevron-right" />
            </div>
          </div>
        </div>
      </div>
    </template>
  </div>
</template>
