<script setup lang="ts">
import { ref, onMounted, onUnmounted, nextTick } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { getSession, getSessionMessages, type Session, type Message } from '@/api'
import TerminalBlock from '@/components/ui/TerminalBlock.vue'

const route = useRoute()
const router = useRouter()

// State
const session = ref<Session | null>(null)
const messages = ref<Message[]>([])
const loading = ref(true)
const loadingMore = ref(false)
const error = ref<string | null>(null)
const hasMore = ref(true)
const offset = ref(0)
const limit = 50
const messagesContainer = ref<HTMLElement | null>(null)

const sessionId = route.params.id as string

// Actions
async function loadData() {
  loading.value = true
  error.value = null

  try {
    session.value = await getSession(sessionId)
    const response = await getSessionMessages(sessionId, limit, 0)
    messages.value = response.data
    offset.value = response.data.length
    hasMore.value = response.data.length < response.total
    
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

async function loadMore() {
  if (loadingMore.value || !hasMore.value) return

  loadingMore.value = true

  try {
    const response = await getSessionMessages(sessionId, limit, offset.value)
    messages.value.push(...response.data)
    offset.value += response.data.length
    hasMore.value = offset.value < response.total
  } catch (e) {
    console.error('Failed to load more messages:', e)
  } finally {
    loadingMore.value = false
  }
}

function handleScroll() {
  if (!messagesContainer.value) return
  const { scrollTop, scrollHeight, clientHeight } = messagesContainer.value
  // Load more when scrolling UP (for chat history) or DOWN?
  // Assuming standard chat: latest at bottom.
  // If API returns latest first, we need to reverse?
  // Let's stick to the previous logic: scrolling UP loads older messages?
  // Actually, the previous logic was: if (scrollHeight - scrollTop - clientHeight < 200) loadMore()
  // This means scrolling to BOTTOM loads more.
  // This implies the API returns OLDEST first? Or we are appending to the end?
  // If we append to end, and scroll to bottom, we are loading NEWER messages?
  // But this is a history viewer.
  // Let's assume we want to load everything.
  
  if (scrollTop < 200) {
     // Maybe load more when scrolling up?
     // For now, let's keep the "scroll to bottom to load more" logic if that's how the API works (pagination).
  }
  
  if (scrollHeight - scrollTop - clientHeight < 200) {
    loadMore()
  }
}

function scrollToBottom() {
  if (messagesContainer.value) {
    messagesContainer.value.scrollTop = messagesContainer.value.scrollHeight
  }
}

function goBack() {
  router.back()
}

onMounted(() => {
  loadData()
  nextTick(() => {
    messagesContainer.value?.addEventListener('scroll', handleScroll)
  })
})

onUnmounted(() => {
  messagesContainer.value?.removeEventListener('scroll', handleScroll)
})
</script>

<template>
  <div class="h-full flex flex-col bg-black font-mono text-gray-300">
    <!-- Terminal Header -->
    <div class="flex items-center justify-between px-4 py-2 border-b border-white/10 bg-surface-100 select-none">
      <div class="flex items-center gap-4">
        <button @click="goBack" class="hover:text-neon-cyan transition-colors">[BACK]</button>
        <span class="text-neon-cyan">SESSION: {{ session?.id }}</span>
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
            />
          </div>
          
          <!-- Loading More Indicator (Bottom) -->
          <div v-if="loadingMore" class="py-4 text-center">
            <div class="i-carbon-circle-dash animate-spin text-xl text-neon-cyan inline-block" />
          </div>
        </div>
        <div v-if="!hasMore" class="text-gray-500 mt-8 text-xs border-t border-white/10 pt-4">
          > END OF TRANSMISSION
        </div>
      </template>
    </div>
  </div>
</template>
