<script setup lang="ts">
import { ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { search, getStats, type SearchResult, type Stats } from '@/api'

const router = useRouter()

// 搜索关键词
const query = ref('')
// 搜索结果
const results = ref<SearchResult[]>([])
// 加载状态
const loading = ref(false)
// 统计信息
const stats = ref<Stats | null>(null)
// 是否已搜索过
const hasSearched = ref(false)

// 防抖定时器
let debounceTimer: ReturnType<typeof setTimeout> | null = null

// 监听搜索输入，实现防抖搜索
watch(query, (newQuery) => {
  if (debounceTimer) {
    clearTimeout(debounceTimer)
  }

  if (!newQuery.trim()) {
    results.value = []
    hasSearched.value = false
    return
  }

  debounceTimer = setTimeout(async () => {
    await performSearch(newQuery)
  }, 300)
})

// 执行搜索
async function performSearch(q: string) {
  if (!q.trim()) return

  loading.value = true
  hasSearched.value = true

  try {
    results.value = await search(q)
  } catch (error) {
    console.error('搜索失败:', error)
    results.value = []
  } finally {
    loading.value = false
  }
}

// 加载统计信息
async function loadStats() {
  try {
    stats.value = await getStats()
  } catch (error) {
    console.error('加载统计信息失败:', error)
  }
}

// 跳转到会话详情
function goToSession(result: SearchResult) {
  router.push(`/sessions/${result.sessionId}`)
}

// 高亮关键词
function highlightKeyword(text: string, keyword: string): string {
  if (!keyword.trim()) return text
  const regex = new RegExp(`(${escapeRegex(keyword)})`, 'gi')
  return text.replace(regex, '<mark class="bg-yellow-500/30 text-yellow-200 px-0.5 rounded">$1</mark>')
}

// 转义正则特殊字符
function escapeRegex(str: string): string {
  return str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

// 截取内容预览
function truncateContent(content: string, maxLength = 200): string {
  if (content.length <= maxLength) return content
  return content.slice(0, maxLength) + '...'
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

// 初始化加载统计
loadStats()
</script>

<template>
  <div class="flex flex-col items-center">
    <!-- 搜索区域 -->
    <div class="w-full max-w-2xl mt-20 mb-8">
      <!-- Logo 和标题 -->
      <div class="flex flex-col items-center mb-8">
        <div class="i-carbon-data-base text-6xl text-blue-400 mb-4" />
        <h1 class="text-3xl font-bold text-gray-100">Memex</h1>
        <p class="text-gray-400 mt-2">Claude Code 会话历史管理</p>
      </div>

      <!-- 搜索框 -->
      <div class="relative">
        <div class="absolute left-4 top-1/2 -translate-y-1/2 i-carbon-search text-xl text-gray-400" />
        <input
          v-model="query"
          type="text"
          placeholder="搜索会话内容..."
          class="w-full h-14 pl-12 pr-4 bg-gray-800 border border-gray-700 rounded-xl text-lg text-gray-100 placeholder-gray-500 focus:outline-none focus:border-blue-500 focus:ring-2 focus:ring-blue-500/20 transition-all"
        />
        <!-- 加载指示器 -->
        <div
          v-if="loading"
          class="absolute right-4 top-1/2 -translate-y-1/2 i-carbon-circle-dash animate-spin text-xl text-blue-400"
        />
      </div>

      <!-- 统计信息 -->
      <div v-if="stats && !hasSearched" class="flex justify-center gap-8 mt-6 text-sm text-gray-500">
        <span>{{ stats.projectCount }} 个项目</span>
        <span>{{ stats.sessionCount }} 个会话</span>
        <span>{{ stats.messageCount }} 条消息</span>
      </div>
    </div>

    <!-- 搜索结果 -->
    <div class="w-full max-w-3xl">
      <!-- 无结果提示 -->
      <div v-if="hasSearched && !loading && results.length === 0" class="text-center text-gray-500 py-8">
        <div class="i-carbon-search text-4xl mb-4 mx-auto opacity-50" />
        <p>没有找到相关结果</p>
      </div>

      <!-- 结果列表 -->
      <div v-else class="space-y-4">
        <div
          v-for="result in results"
          :key="result.messageId"
          class="p-4 bg-gray-800 rounded-lg border border-gray-700 hover:border-gray-600 cursor-pointer transition-colors"
          @click="goToSession(result)"
        >
          <!-- 会话 ID 和时间 -->
          <div class="flex items-center justify-between text-sm text-gray-500 mb-2">
            <span class="flex items-center gap-1 truncate max-w-[70%]">
              <div class="i-carbon-chat" />
              {{ result.sessionId.slice(0, 8) }}...
            </span>
            <span>{{ formatTime(result.timestamp) }}</span>
          </div>

          <!-- 角色标签 -->
          <div class="flex items-center gap-2 mb-2">
            <span
              class="px-2 py-0.5 rounded text-xs"
              :class="result.type === 'user' ? 'bg-green-900 text-green-300' : 'bg-blue-900 text-blue-300'"
            >
              {{ result.type === 'user' ? '用户' : 'Claude' }}
            </span>
          </div>

          <!-- 内容预览（高亮关键词） -->
          <p
            class="text-gray-300 text-sm leading-relaxed"
            v-html="highlightKeyword(truncateContent(result.snippet || result.content), query)"
          />
        </div>
      </div>
    </div>
  </div>
</template>
