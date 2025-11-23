<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { getProjects, type Project } from '@/api'

const router = useRouter()

// 项目列表
const projects = ref<Project[]>([])
// 加载状态
const loading = ref(true)
// 错误信息
const error = ref<string | null>(null)

// 加载项目列表
async function loadProjects() {
  loading.value = true
  error.value = null

  try {
    projects.value = await getProjects()
  } catch (e) {
    console.error('加载项目列表失败:', e)
    error.value = '加载失败，请稍后重试'
  } finally {
    loading.value = false
  }
}

// 跳转到项目详情
function goToProject(project: Project) {
  router.push(`/projects/${project.id}`)
}

// 从路径中提取项目名称
function getProjectName(path: string): string {
  const parts = path.split('/')
  return parts[parts.length - 1] || path
}

// 格式化时间
function formatTime(timestamp: string): string {
  const date = new Date(timestamp)
  return date.toLocaleDateString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  })
}

onMounted(() => {
  loadProjects()
})
</script>

<template>
  <div>
    <!-- 页面标题 -->
    <div class="flex items-center justify-between mb-6">
      <h1 class="text-2xl font-bold">项目列表</h1>
      <span class="text-gray-500 text-sm">共 {{ projects.length }} 个项目</span>
    </div>

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
        @click="loadProjects"
      >
        重试
      </button>
    </div>

    <!-- 空状态 -->
    <div v-else-if="projects.length === 0" class="text-center py-12">
      <div class="i-carbon-folder text-4xl text-gray-500 mb-4 mx-auto" />
      <p class="text-gray-400">暂无项目数据</p>
    </div>

    <!-- 项目列表 -->
    <div v-else class="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
      <div
        v-for="project in projects"
        :key="project.id"
        class="p-4 bg-gray-800 rounded-lg border border-gray-700 hover:border-gray-600 cursor-pointer transition-all hover:shadow-lg"
        @click="goToProject(project)"
      >
        <!-- 项目名称 -->
        <div class="flex items-center gap-2 mb-2">
          <div class="i-carbon-folder text-xl text-blue-400" />
          <h3 class="font-semibold truncate">{{ getProjectName(project.path) }}</h3>
        </div>

        <!-- 完整路径 -->
        <p class="text-sm text-gray-500 truncate mb-3" :title="project.path">
          {{ project.path }}
        </p>

        <!-- 统计信息 -->
        <div class="flex items-center justify-between text-sm text-gray-400">
          <span class="flex items-center gap-1">
            <div class="i-carbon-chat" />
            {{ project.sessionCount }} 个会话
          </span>
          <span>{{ formatTime(project.updatedAt) }}</span>
        </div>
      </div>
    </div>
  </div>
</template>
