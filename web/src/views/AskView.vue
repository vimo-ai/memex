<script setup lang="ts">
import { ref, nextTick, watch } from 'vue'

defineOptions({
  name: 'AskView'
})
import { useRouter } from 'vue-router'
import { ask, type RagResponse } from '@/api'
import { renderMarkdown } from '@/utils/markdown'
import GlitchText from '@/components/ui/GlitchText.vue'

const router = useRouter()

// State
const question = ref('')
const loading = ref(false)
const conversation = ref<Array<{
  type: 'user' | 'ai'
  content: string
  sources?: RagResponse['sources']
  model?: string
  timestamp: number
}>>([])

const inputRef = ref<HTMLInputElement | null>(null)
const messagesEndRef = ref<HTMLDivElement | null>(null)

// Auto-scroll to bottom
watch(conversation, () => {
  nextTick(() => {
    messagesEndRef.value?.scrollIntoView({ behavior: 'smooth' })
  })
}, { deep: true })

async function handleAsk() {
  if (!question.value.trim() || loading.value) return

  const currentQuestion = question.value
  question.value = ''
  loading.value = true

  // Add user message
  conversation.value.push({
    type: 'user',
    content: currentQuestion,
    timestamp: Date.now()
  })

  try {
    const response = await ask({
      question: currentQuestion,
      contextWindow: 5, // Slightly larger context
      maxSources: 5
    })

    // Add AI response
    conversation.value.push({
      type: 'ai',
      content: response.answer,
      sources: response.sources,
      model: response.model,
      timestamp: Date.now()
    })
  } catch (error) {
    console.error('Ask failed:', error)
    conversation.value.push({
      type: 'ai',
      content: '⚠️ SYSTEM ERROR: Neural Link Disconnected. Please try again.',
      timestamp: Date.now()
    })
  } finally {
    loading.value = false
    nextTick(() => {
      inputRef.value?.focus()
    })
  }
}

function goToSession(sessionId: string, messageIndex: number) {
  router.push(`/sessions/${sessionId}?highlight=${messageIndex}`)
}

function formatTime(timestamp: number): string {
  return new Date(timestamp).toLocaleTimeString()
}
</script>

<template>
  <div class="h-full flex flex-col relative overflow-hidden">
    
    <!-- Background Elements -->
    <div class="absolute inset-0 pointer-events-none">
      <div class="absolute top-0 left-1/4 w-[500px] h-[500px] bg-neon-cyan/5 rounded-full blur-[100px]" />
      <div class="absolute bottom-0 right-1/4 w-[500px] h-[500px] bg-neon-violet/5 rounded-full blur-[100px]" />
    </div>

    <!-- Header -->
    <div class="flex-none p-6 border-b border-white/5 bg-surface-100/80 backdrop-blur-md z-10 flex items-center justify-between">
      <div class="flex items-center gap-4">
        <GlitchText text="NEURAL LINK" class="text-2xl font-bold text-white" />
        <span class="text-xs font-mono text-neon-cyan px-2 py-0.5 border border-neon-cyan/30 rounded bg-neon-cyan/5">ONLINE</span>
      </div>
      <div class="text-xs font-mono text-gray-500">
        MEMEX RAG SYSTEM v1.0
      </div>
    </div>

    <!-- Messages Area -->
    <div class="flex-1 overflow-y-auto p-6 space-y-8 scroll-smooth">
      
      <!-- Welcome Message -->
      <div v-if="conversation.length === 0" class="h-full flex flex-col items-center justify-center opacity-50">
        <div class="i-carbon-ibm-watson-discovery text-6xl text-neon-cyan mb-4 animate-pulse" />
        <p class="font-mono text-neon-cyan">AWAITING INPUT...</p>
      </div>

      <!-- Message Stream -->
      <div v-for="(msg, idx) in conversation" :key="idx" class="max-w-4xl mx-auto w-full animate-fade-in-up">
        
        <!-- User Message -->
        <div v-if="msg.type === 'user'" class="flex justify-end mb-8">
          <div class="max-w-[80%]">
            <div class="flex items-center justify-end gap-2 mb-2">
              <span class="text-xs font-mono text-gray-500">{{ formatTime(msg.timestamp) }}</span>
              <span class="text-xs font-mono text-neon-cyan">USER</span>
            </div>
            <div class="bg-surface-200/50 border border-neon-cyan/30 p-4 rounded-lg rounded-tr-none text-white font-mono text-sm shadow-[0_0_15px_rgba(0,243,255,0.1)]">
              {{ msg.content }}
            </div>
          </div>
        </div>

        <!-- AI Message -->
        <div v-else class="flex justify-start">
          <div class="max-w-[90%] w-full">
            <div class="flex items-center gap-2 mb-2">
              <span class="text-xs font-mono text-neon-violet">AI_CORE</span>
              <span class="text-xs font-mono text-gray-500">{{ formatTime(msg.timestamp) }}</span>
              <span v-if="msg.model" class="text-[10px] font-mono text-gray-600 border border-gray-700 px-1 rounded">{{ msg.model }}</span>
            </div>
            
            <div class="bg-surface-100/80 border border-white/10 p-6 rounded-lg rounded-tl-none relative overflow-hidden group">
              <!-- Content -->
              <div class="prose prose-invert prose-sm max-w-none font-sans leading-relaxed" v-html="renderMarkdown(msg.content)" />

              <!-- Sources -->
              <div v-if="msg.sources && msg.sources.length > 0" class="mt-6 pt-4 border-t border-white/5">
                <div class="text-xs font-mono text-gray-500 mb-3 flex items-center gap-2">
                  <div class="i-carbon-data-reference" />
                  DATA SOURCES
                </div>
                <div class="grid grid-cols-1 md:grid-cols-2 gap-3">
                  <button
                    v-for="(source, sIdx) in msg.sources"
                    :key="sIdx"
                    @click="goToSession(source.sessionId, source.messageIndex)"
                    class="text-left p-3 bg-black/20 border border-white/5 hover:border-neon-cyan/30 hover:bg-neon-cyan/5 rounded transition-all group/source"
                  >
                    <div class="flex items-center justify-between mb-1">
                      <span class="text-xs font-mono text-neon-cyan truncate max-w-[150px]">{{ source.projectName }}</span>
                      <span class="text-[10px] font-mono text-gray-600">{{ (source.score * 100).toFixed(0) }}% MATCH</span>
                    </div>
                    <div class="text-xs text-gray-400 line-clamp-2 font-mono opacity-70 group-hover/source:opacity-100 transition-opacity">
                      {{ source.snippet }}
                    </div>
                  </button>
                </div>
              </div>

              <!-- Decorative Corner -->
              <div class="absolute top-0 left-0 w-2 h-2 border-t border-l border-neon-violet opacity-50" />
            </div>
          </div>
        </div>
      </div>

      <!-- Loading State -->
      <div v-if="loading" class="max-w-4xl mx-auto w-full">
        <div class="flex justify-start">
          <div class="bg-surface-100/50 border border-white/5 p-4 rounded-lg rounded-tl-none flex items-center gap-3">
            <div class="flex gap-1">
              <div class="w-1.5 h-1.5 bg-neon-cyan rounded-full animate-bounce" style="animation-delay: 0ms" />
              <div class="w-1.5 h-1.5 bg-neon-cyan rounded-full animate-bounce" style="animation-delay: 150ms" />
              <div class="w-1.5 h-1.5 bg-neon-cyan rounded-full animate-bounce" style="animation-delay: 300ms" />
            </div>
            <span class="text-xs font-mono text-neon-cyan animate-pulse">PROCESSING...</span>
          </div>
        </div>
      </div>

      <div ref="messagesEndRef" />
    </div>

    <!-- Input Area -->
    <div class="flex-none p-6 bg-surface-100/80 backdrop-blur-xl border-t border-white/5 z-20">
      <div class="max-w-4xl mx-auto relative group">
        <div class="absolute inset-0 bg-gradient-to-r from-neon-cyan/20 to-neon-violet/20 rounded-lg blur-md opacity-0 group-hover:opacity-100 transition-opacity duration-500" />
        
        <div class="relative flex items-center bg-black/40 border border-white/10 group-hover:border-neon-cyan/30 rounded-lg transition-colors overflow-hidden">
          <input
            ref="inputRef"
            v-model="question"
            type="text"
            placeholder="ASK A QUESTION..."
            class="w-full bg-transparent border-none focus:ring-0 focus:outline-none text-white placeholder-gray-600 px-6 py-4 font-mono"
            :disabled="loading"
            @keydown.enter="handleAsk"
            autofocus
          />
          <button 
            @click="handleAsk"
            :disabled="!question.trim() || loading"
            class="px-6 py-4 text-neon-cyan hover:text-white hover:bg-neon-cyan/20 transition-colors disabled:opacity-30 disabled:cursor-not-allowed"
          >
            <div class="i-carbon-send-alt text-xl" />
          </button>
        </div>
      </div>
      <div class="max-w-4xl mx-auto mt-2 text-center">
        <p class="text-[10px] text-gray-600 font-mono">AI CAN MAKE MISTAKES. VERIFY WITH SOURCES.</p>
      </div>
    </div>

  </div>
</template>

<style scoped>
/* Custom Scrollbar */
::-webkit-scrollbar {
  width: 6px;
}
::-webkit-scrollbar-track {
  background: transparent;
}
::-webkit-scrollbar-thumb {
  background: rgba(255, 255, 255, 0.1);
  border-radius: 3px;
}
::-webkit-scrollbar-thumb:hover {
  background: rgba(0, 243, 255, 0.3);
}
</style>
