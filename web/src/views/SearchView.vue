<script setup lang="ts">
import { ref, watch, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { search, getStats, type SearchResult, type Stats } from '@/api'
import GlitchText from '@/components/ui/GlitchText.vue'

const router = useRouter()

// State
const query = ref('')
const results = ref<SearchResult[]>([])
const loading = ref(false)
const stats = ref<Stats | null>(null)
const hasSearched = ref(false)

let debounceTimer: ReturnType<typeof setTimeout> | null = null

// Watchers
watch(query, (newQuery) => {
  if (debounceTimer) clearTimeout(debounceTimer)

  if (!newQuery.trim()) {
    results.value = []
    hasSearched.value = false
    return
  }

  debounceTimer = setTimeout(async () => {
    await performSearch(newQuery)
  }, 300)
})

// Actions
async function performSearch(q: string) {
  if (!q.trim()) return
  loading.value = true
  hasSearched.value = true

  try {
    results.value = await search(q)
  } catch (error) {
    console.error('Search failed:', error)
    results.value = []
  } finally {
    loading.value = false
  }
}

async function loadStats() {
  try {
    stats.value = await getStats()
  } catch (error) {
    console.error('Failed to load stats:', error)
  }
}

function goToSession(result: SearchResult) {
  router.push(`/sessions/${result.sessionId}`)
}

function highlightKeyword(text: string, keyword: string): string {
  if (!keyword.trim()) return text
  const regex = new RegExp(`(${escapeRegex(keyword)})`, 'gi')
  return text.replace(regex, '<mark class="bg-neon-cyan/20 text-neon-cyan px-0.5 rounded font-medium shadow-[0_0_10px_rgba(0,243,255,0.2)]">$1</mark>')
}

function escapeRegex(str: string): string {
  return str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

function truncateContent(content: string, maxLength = 200): string {
  if (content.length <= maxLength) return content
  return content.slice(0, maxLength) + '...'
}

function formatTime(timestamp: string): string {
  return new Date(timestamp).toLocaleString()
}

onMounted(() => {
  loadStats()
})
</script>

<template>
  <div class="h-full flex flex-col items-center justify-center relative transition-all duration-700" :class="{ 'justify-start pt-20': hasSearched }">
    
    <!-- The Core (Animated Background) -->
    <div 
      class="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 w-[600px] h-[600px] bg-neon-cyan/5 rounded-full blur-[100px] pointer-events-none transition-all duration-1000"
      :class="{ 'w-[800px] h-[300px] -translate-y-full opacity-20': hasSearched }"
    />

    <!-- Search Container -->
    <div class="w-full max-w-4xl px-6 z-10 flex flex-col items-center">
      
      <!-- Title -->
      <div class="mb-12 text-center transition-all duration-500" :class="{ 'opacity-0 h-0 overflow-hidden mb-0': hasSearched }">
        <div class="mb-4">
          <GlitchText text="MEMEX ORACLE" class="text-5xl font-bold text-white" />
        </div>
        <p class="text-gray-500 font-mono text-sm tracking-widest">ACCESSING NEURAL ARCHIVES...</p>
      </div>

      <!-- Search Input -->
      <div class="w-full relative group">
        <div class="absolute inset-0 bg-gradient-to-r from-neon-cyan/20 to-neon-violet/20 rounded-none blur-md opacity-0 group-hover:opacity-100 transition-opacity duration-500" />
        <div class="relative flex items-center bg-surface-100/80 backdrop-blur-xl border border-white/10 group-hover:border-neon-cyan/30 transition-colors">
          <div class="pl-6 text-neon-cyan">
            <div class="i-carbon-search text-2xl" />
          </div>
          <input
            v-model="query"
            type="text"
            placeholder="ENTER QUERY COMMAND..."
            class="w-full bg-transparent border-none focus:ring-0 text-xl font-mono text-white placeholder-gray-600 px-6 py-6 uppercase tracking-wider"
            autofocus
          />
          <div v-if="loading" class="pr-6">
            <div class="i-carbon-circle-dash animate-spin text-2xl text-neon-cyan" />
          </div>
        </div>
        
        <!-- Corner Accents -->
        <div class="absolute top-0 left-0 w-2 h-2 border-t border-l border-neon-cyan opacity-50" />
        <div class="absolute top-0 right-0 w-2 h-2 border-t border-r border-neon-cyan opacity-50" />
        <div class="absolute bottom-0 left-0 w-2 h-2 border-b border-l border-neon-cyan opacity-50" />
        <div class="absolute bottom-0 right-0 w-2 h-2 border-b border-r border-neon-cyan opacity-50" />
      </div>

      <!-- Stats (Hidden when searching) -->
      <div v-if="stats && !hasSearched" class="flex gap-12 mt-16 font-mono text-xs text-gray-500">
        <div class="flex flex-col items-center gap-2">
          <span class="text-2xl text-white font-display">{{ stats.projectCount }}</span>
          <span class="tracking-widest">NODES</span>
        </div>
        <div class="flex flex-col items-center gap-2">
          <span class="text-2xl text-white font-display">{{ stats.sessionCount }}</span>
          <span class="tracking-widest">LINKS</span>
        </div>
        <div class="flex flex-col items-center gap-2">
          <span class="text-2xl text-white font-display">{{ stats.messageCount }}</span>
          <span class="tracking-widest">DATA</span>
        </div>
      </div>
    </div>

    <!-- Results Stream -->
    <div v-if="hasSearched" class="w-full max-w-4xl px-6 mt-8 pb-32 overflow-y-auto scroll-smooth h-full">
      <div v-if="!loading && results.length === 0" class="text-center py-20 opacity-50">
        <div class="font-mono text-neon-cyan mb-2">NO MATCHES FOUND</div>
        <div class="text-xs text-gray-600">TRY ADJUSTING QUERY PARAMETERS</div>
      </div>

      <div v-else class="space-y-4">
        <div
          v-for="result in results"
          :key="result.messageId"
          class="group relative bg-surface-100/50 border border-white/5 p-6 cursor-pointer hover:bg-surface-200/50 hover:border-neon-cyan/30 transition-all duration-300"
          @click="goToSession(result)"
        >
          <!-- Hover Glow -->
          <div class="absolute inset-0 bg-neon-cyan/5 opacity-0 group-hover:opacity-100 transition-opacity pointer-events-none" />

          <div class="relative z-10">
            <div class="flex items-center justify-between mb-3 font-mono text-xs">
              <div class="flex items-center gap-3">
                <span :class="result.type === 'user' ? 'text-neon-cyan' : 'text-neon-violet'">
                  {{ result.type === 'user' ? 'USER' : 'SYSTEM' }}
                </span>
                <span class="text-gray-600">::</span>
                <span class="text-gray-500">{{ result.sessionId.slice(0, 8) }}</span>
              </div>
              <span class="text-gray-600">{{ formatTime(result.timestamp) }}</span>
            </div>

            <div 
              class="font-mono text-sm text-gray-300 leading-relaxed line-clamp-3"
              v-html="highlightKeyword(truncateContent(result.snippet || result.content), query)"
            />
          </div>
        </div>
      </div>
    </div>

  </div>
</template>
