<script setup lang="ts">
import { ref, onMounted, nextTick } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { getSession, getSessionMessages, type Session, type Message } from '@/api'
import TerminalBlock from '@/components/ui/TerminalBlock.vue'

const route = useRoute()
const router = useRouter()

// State
const session = ref<Session | null>(null)
const messages = ref<Message[]>([])
const loading = ref(true)
const error = ref<string | null>(null)
const messagesContainer = ref<HTMLElement | null>(null)

// 支持两种路由模式
const sessionId = (route.params.sessionId || route.params.id) as string
const projectId = route.params.projectId as string | undefined

// 判断来源：从项目页还是搜索页
const isFromProject = !!projectId

// Actions
async function loadData() {
  loading.value = true
  error.value = null

  try {
    session.value = await getSession(sessionId)
    const response = await getSessionMessages(sessionId)
    messages.value = response.messages

    nextTick(() => {
      scrollToBottom()
    })
  } catch (e) {
    console.error('Failed to load session data:', e)
    error.value = 'CONNECTION REFUSED'
  } finally {
    loading.value = false
  }
}

function scrollToBottom() {
  if (messagesContainer.value) {
    messagesContainer.value.scrollTop = messagesContainer.value.scrollHeight
  }
}

function goBack() {
  if (isFromProject) {
    // 从项目页进入，返回项目详情
    router.push(`/projects/${projectId}`)
  } else {
    // 从搜索页进入，返回搜索页（保留搜索状态）
    router.push('/search')
  }
}

onMounted(() => {
  loadData()
})
</script>

<template>
  <div class="h-full flex flex-col bg-black font-mono text-gray-300">
    <!-- Terminal Header -->
    <div class="flex items-center justify-between px-4 py-2 border-b border-white/10 bg-surface-100 select-none">
      <div class="flex items-center gap-4">
        <button @click="goBack" class="hover:text-neon-cyan transition-colors">
          [{{ isFromProject ? 'PROJECT' : 'SEARCH' }}]
        </button>
        <span class="text-neon-cyan">SESSION: {{ session?.id }}</span>
        <span v-if="session" class="inline-flex items-center px-2 py-0.5 rounded bg-white/5 border border-white/10 text-[10px] uppercase tracking-wide">
          <span :style="{ color: (session.source || 'claude').toLowerCase() === 'claude' ? '#D97757' : '#00f3ff' }">
            {{ (session.source || 'claude').toUpperCase() }}
          </span>
        </span>
        <span v-if="session?.cwd" class="text-[10px] text-gray-500 bg-white/5 border border-white/5 px-2 py-0.5 rounded">
          {{ session.cwd }}
        </span>
      </div>
      <div class="flex gap-4 text-xs text-gray-500">
        <span>STATUS: READ_ONLY</span>
        <span>ENCRYPTION: NONE</span>
      </div>
    </div>

    <!-- Terminal Output -->
    <div 
      ref="messagesContainer"
      class="flex-1 overflow-y-auto p-4 scroll-smooth"
    >
      <div v-if="loading" class="text-neon-cyan animate-pulse">
        > ESTABLISHING CONNECTION...
      </div>

      <div v-else-if="error" class="text-red-500">
        > ERROR: {{ error }}
      </div>

      <template v-else>
        <div class="text-gray-500 mb-8 text-xs">
          > SYSTEM INITIALIZED<br>
          > LOADED {{ messages.length }} PACKETS<br>
          > BEGIN TRANSMISSION...
        </div>
        <div class="flex flex-col">
          <div 
            v-for="msg in messages" 
            :key="msg.id"
            class="flex w-full"
            :class="msg.type === 'user' ? 'justify-end' : 'justify-start'"
          >
            <TerminalBlock
              :content="msg.content"
              :type="msg.type"
              :timestamp="msg.timestamp"
              :tool-name="msg.toolName"
              :tool-args="msg.toolArgs"
            />
          </div>
          
        </div>
        <div class="text-gray-500 mt-8 text-xs border-t border-white/10 pt-4">
          > END OF TRANSMISSION
        </div>
      </template>
    </div>
  </div>
</template>
