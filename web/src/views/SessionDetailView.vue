<script setup lang="ts">
import { ref, onMounted, onUnmounted, nextTick } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { getSession, getSessionMessages, type Session, type Message } from '@/api'

const route = useRoute()
const router = useRouter()

// 会话信息
const session = ref<Session | null>(null)
// 消息列表
const messages = ref<Message[]>([])
// 加载状态
const loading = ref(true)
// 加载更多状态
const loadingMore = ref(false)
// 错误信息
const error = ref<string | null>(null)
// 是否还有更多消息
const hasMore = ref(true)
// 分页偏移量
const offset = ref(0)
// 每页数量
const limit = 50
// 消息容器引用
const messagesContainer = ref<HTMLElement | null>(null)

// 获取会话 ID
const sessionId = Number(route.params.id)

// 加载数据
async function loadData() {
  loading.value = true
  error.value = null

  try {
    // 加载会话信息
    session.value = await getSession(sessionId)

    // 加载消息列表
    const response = await getSessionMessages(sessionId, limit, 0)
    messages.value = response.data
    offset.value = response.data.length
    hasMore.value = response.data.length < response.total
  } catch (e) {
    console.error('加载数据失败:', e)
    error.value = '加载失败，请稍后重试'
  } finally {
    loading.value = false
  }
}

// 加载更多消息
async function loadMore() {
  if (loadingMore.value || !hasMore.value) return

  loadingMore.value = true

  try {
    const response = await getSessionMessages(sessionId, limit, offset.value)
    messages.value.push(...response.data)
    offset.value += response.data.length
    hasMore.value = offset.value < response.total
  } catch (e) {
    console.error('加载更多消息失败:', e)
  } finally {
    loadingMore.value = false
  }
}

// 滚动监听（实现滚动加载更多）
function handleScroll() {
  if (!messagesContainer.value) return

  const { scrollTop, scrollHeight, clientHeight } = messagesContainer.value
  // 距离底部 200px 时加载更多
  if (scrollHeight - scrollTop - clientHeight < 200) {
    loadMore()
  }
}

// 返回上一页
function goBack() {
  router.back()
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
    second: '2-digit',
  })
}

// 处理消息内容（简单的换行处理）
function formatContent(content: string): string {
  return content
}

onMounted(() => {
  loadData()
  // 添加滚动监听
  nextTick(() => {
    messagesContainer.value?.addEventListener('scroll', handleScroll)
  })
})

onUnmounted(() => {
  // 移除滚动监听
  messagesContainer.value?.removeEventListener('scroll', handleScroll)
})
</script>

<template>
  <div class="h-[calc(100vh-7rem)] flex flex-col">
    <!-- 加载状态 -->
    <div v-if="loading" class="flex-1 flex justify-center items-center">
      <div class="i-carbon-circle-dash animate-spin text-3xl text-blue-400" />
    </div>

    <!-- 错误提示 -->
    <div v-else-if="error" class="flex-1 flex flex-col justify-center items-center">
      <div class="i-carbon-warning text-4xl text-red-400 mb-4" />
      <p class="text-gray-400">{{ error }}</p>
      <button
        class="mt-4 px-4 py-2 bg-blue-600 hover:bg-blue-500 rounded-lg transition-colors"
        @click="loadData"
      >
        重试
      </button>
    </div>

    <template v-else-if="session">
      <!-- 会话头部 -->
      <div class="bg-gray-800 rounded-lg border border-gray-700 p-4 mb-4 flex-shrink-0">
        <div class="flex items-center gap-4">
          <button
            class="p-2 hover:bg-gray-700 rounded-lg transition-colors"
            @click="goBack"
          >
            <div class="i-carbon-arrow-left text-lg" />
          </button>
          <div>
            <h1 class="font-semibold">会话详情</h1>
            <div class="flex items-center gap-4 text-sm text-gray-500 mt-1">
              <span class="font-mono">{{ session.uuid.slice(0, 8) }}...</span>
              <span>{{ session.messageCount }} 条消息</span>
              <span>{{ formatTime(session.createdAt) }}</span>
            </div>
          </div>
        </div>
      </div>

      <!-- 消息列表 -->
      <div
        ref="messagesContainer"
        class="flex-1 overflow-y-auto space-y-4 pr-2"
      >
        <div
          v-for="message in messages"
          :key="message.id"
          class="flex"
          :class="message.role === 'user' ? 'justify-end' : 'justify-start'"
        >
          <div
            class="max-w-[80%] rounded-lg p-4"
            :class="message.role === 'user'
              ? 'bg-blue-900/50 border border-blue-800'
              : 'bg-gray-800 border border-gray-700'"
          >
            <!-- 角色标签和时间 -->
            <div class="flex items-center gap-2 mb-2 text-xs">
              <span
                class="px-2 py-0.5 rounded"
                :class="message.role === 'user' ? 'bg-green-900 text-green-300' : 'bg-blue-900 text-blue-300'"
              >
                {{ message.role === 'user' ? '用户' : 'Claude' }}
              </span>
              <span class="text-gray-500">{{ formatTime(message.timestamp) }}</span>
            </div>

            <!-- 消息内容 -->
            <div class="text-sm text-gray-200 whitespace-pre-wrap break-words">
              {{ formatContent(message.content) }}
            </div>
          </div>
        </div>

        <!-- 加载更多指示器 -->
        <div v-if="loadingMore" class="flex justify-center py-4">
          <div class="i-carbon-circle-dash animate-spin text-xl text-blue-400" />
        </div>

        <!-- 已加载全部 -->
        <div v-else-if="!hasMore && messages.length > 0" class="text-center py-4 text-gray-500 text-sm">
          已加载全部消息
        </div>
      </div>
    </template>
  </div>
</template>
