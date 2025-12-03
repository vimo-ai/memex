<script setup lang="ts">
import { ref, watch, onMounted, onUnmounted } from 'vue'
import { useRouter } from 'vue-router'
import { semanticSearch, getStats, getEmbeddingStats, searchSessionsByIdPrefix, getProjects, type SearchResult, type Stats, type SemanticSearchResult, type SemanticSearchMode, type EmbeddingStats, type Session, type Project } from '@/api'
import GlitchText from '@/components/ui/GlitchText.vue'

const router = useRouter()

// State
const query = ref('')
const results = ref<(SearchResult | SemanticSearchResult)[]>([])
const sessionResults = ref<Session[]>([])
const loading = ref(false)
const stats = ref<Stats | null>(null)
const embeddingStats = ref<EmbeddingStats | null>(null)
const hasSearched = ref(false)

// 搜索类型：content（全文搜索）、sessionId（Session ID 搜索）
const searchType = ref<'content' | 'sessionId'>('content')

// 搜索模式：fts（原有）、vector（纯向量）、hybrid（混合）
const searchMode = ref<SemanticSearchMode>('hybrid')

// 过滤器状态
const projects = ref<Project[]>([])
const selectedProjectId = ref<number | undefined>(undefined)
const startDate = ref<string>('')
const endDate = ref<string>('')
const showProjectDropdown = ref(false)

let debounceTimer: ReturnType<typeof setTimeout> | null = null

// Watchers
watch(query, (newQuery) => {
  if (debounceTimer) clearTimeout(debounceTimer)

  if (!newQuery.trim()) {
    results.value = []
    sessionResults.value = []
    hasSearched.value = false
    return
  }

  debounceTimer = setTimeout(async () => {
    await performSearch(newQuery)
  }, 300)
})

// 切换搜索类型时清空结果
watch(searchType, () => {
  results.value = []
  sessionResults.value = []
  hasSearched.value = false
  query.value = ''
})

// 切换模式时自动重新搜索
watch(searchMode, () => {
  if (query.value.trim() && searchType.value === 'content') {
    performSearch(query.value)
  }
})

// 过滤器变化时自动重新搜索
watch([selectedProjectId, startDate, endDate], () => {
  if (query.value.trim() && searchType.value === 'content') {
    performSearch(query.value)
  }
})

// Actions
async function performSearch(q: string) {
  if (!q.trim()) return
  loading.value = true
  hasSearched.value = true

  try {
    if (searchType.value === 'content') {
      // 使用语义搜索 API（支持 fts/vector/hybrid 模式）
      results.value = await semanticSearch(q, {
        mode: searchMode.value,
        limit: 20,
        projectId: selectedProjectId.value,
        startDate: startDate.value || undefined,
        endDate: endDate.value || undefined,
      })
      sessionResults.value = []
    } else {
      // Session ID 前缀搜索
      sessionResults.value = await searchSessionsByIdPrefix(q, 20)
      results.value = []
    }
  } catch (error) {
    console.error('Search failed:', error)
    results.value = []
    sessionResults.value = []
  } finally {
    loading.value = false
  }
}

async function loadStats() {
  try {
    stats.value = await getStats()
    embeddingStats.value = await getEmbeddingStats()
  } catch (error) {
    console.error('Failed to load stats:', error)
  }
}

async function loadProjects() {
  try {
    projects.value = await getProjects()
  } catch (error) {
    console.error('Failed to load projects:', error)
  }
}

function goToSession(result: SearchResult | SemanticSearchResult | Session) {
  if ('sessionId' in result) {
    router.push(`/sessions/${result.sessionId}`)
  } else {
    router.push(`/sessions/${result.id}`)
  }
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

function sourceLabel(result: any): string {
  return result.source ? result.source.toUpperCase() : 'CLAUDE'
}

function channelLabel(result: any): string | null {
  return result.channel ? result.channel.toUpperCase() : null
}

onMounted(() => {
  loadStats()
  loadProjects()
  document.addEventListener('click', handleClickOutside)
})

onUnmounted(() => {
  document.removeEventListener('click', handleClickOutside)
})

function handleClickOutside(event: MouseEvent) {
  const target = event.target as HTMLElement
  // Check if the click is outside the project dropdown
  // We use the escaped class name for the selector
  if (showProjectDropdown.value && !target.closest('.group\\/project')) {
    showProjectDropdown.value = false
  }
}

// Helper methods for UI
function toggleProjectDropdown() {
  showProjectDropdown.value = !showProjectDropdown.value
}

function selectProject(id: number | undefined) {
  selectedProjectId.value = id
  showProjectDropdown.value = false
}

function clearFilters() {
  selectedProjectId.value = undefined
  startDate.value = ''
  endDate.value = ''
}
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

      <!-- Search Type Toggle -->
      <div class="flex items-center gap-2 mb-8 font-mono text-xs">
        <span class="text-gray-600 mr-2">SEARCH TYPE:</span>
        <button
          @click="searchType = 'content'"
          class="px-4 py-2 border transition-all uppercase tracking-wider"
          :class="searchType === 'content'
            ? 'border-neon-cyan text-black bg-neon-cyan font-bold'
            : 'border-white/20 text-gray-300 hover:border-white/40 hover:text-white'"
        >
          CONTENT
        </button>
        <button
          @click="searchType = 'sessionId'"
          class="px-4 py-2 border transition-all uppercase tracking-wider"
          :class="searchType === 'sessionId'
            ? 'border-neon-cyan text-black bg-neon-cyan font-bold'
            : 'border-white/20 text-gray-300 hover:border-white/40 hover:text-white'"
        >
          SESSION ID
        </button>
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
            :placeholder="searchType === 'content' ? 'ENTER QUERY COMMAND...' : 'ENTER SESSION ID PREFIX...'"
            class="w-full bg-transparent border-none focus:ring-0 focus:outline-none focus-visible:outline-none text-xl font-mono text-white placeholder-gray-600 px-6 py-6 uppercase tracking-wider"
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

      <!-- Search Mode Toggle (only for content search) -->
      <div v-if="searchType === 'content'" class="flex items-center gap-2 mt-4 font-mono text-xs">
        <span class="text-gray-600 mr-2">MODE:</span>
        <button
          v-for="mode in (['fts', 'hybrid', 'vector'] as const)"
          :key="mode"
          @click="searchMode = mode"
          class="px-3 py-1.5 border transition-all uppercase tracking-wider"
          :class="searchMode === mode
            ? 'border-neon-cyan text-black bg-neon-cyan font-bold'
            : 'border-white/20 text-gray-300 hover:border-white/40 hover:text-white'"
        >
          {{ mode === 'fts' ? 'KEYWORD' : mode === 'vector' ? 'SEMANTIC' : 'HYBRID' }}
        </button>
        <span v-if="embeddingStats" class="ml-4 text-gray-600">
          [{{ embeddingStats.indexedMessages.toLocaleString() }} VECTORS]
        </span>
      </div>

      <!-- Filters (only for content search) -->
      <div v-if="searchType === 'content'" class="w-full mt-8 flex flex-col items-center gap-4 font-mono text-xs animate-fade-in-up">
        
        <!-- Filter Control Deck -->
        <div class="flex items-center gap-4 p-2 bg-surface-100/50 border border-white/10 rounded-lg backdrop-blur-md">
          
          <!-- Project Filter (Custom Dropdown) -->
          <div class="relative group/project">
            <button 
              class="flex items-center gap-3 px-4 py-2 bg-black/40 border border-white/10 hover:border-neon-cyan/50 rounded transition-all min-w-[200px] justify-between group-hover/project:shadow-[0_0_15px_rgba(0,243,255,0.1)]"
              @click="toggleProjectDropdown"
            >
              <div class="flex items-center gap-2">
                <div class="i-carbon-cube text-neon-cyan" />
                <span :class="selectedProjectId ? 'text-white' : 'text-gray-400'">
                  {{ selectedProjectId ? projects.find(p => p.id === selectedProjectId)?.path.split('/').pop() : 'ALL PROJECTS' }}
                </span>
              </div>
              <div class="i-carbon-chevron-down text-gray-500 transition-transform group-hover/project:text-neon-cyan" />
            </button>

            <!-- Dropdown Menu -->
            <div 
              v-if="showProjectDropdown"
              class="absolute top-full left-0 mt-2 w-[300px] max-h-[400px] overflow-y-auto bg-surface-100 border border-neon-cyan/30 rounded shadow-[0_0_30px_rgba(0,0,0,0.8)] backdrop-blur-xl z-50 flex flex-col py-2"
            >
              <button
                class="px-4 py-3 text-left hover:bg-neon-cyan/10 hover:text-neon-cyan transition-colors flex items-center gap-2 border-b border-white/5"
                :class="{ 'text-neon-cyan bg-neon-cyan/5': selectedProjectId === undefined }"
                @click="selectProject(undefined)"
              >
                <div class="i-carbon-apps" />
                ALL PROJECTS
              </button>
              <button
                v-for="project in projects"
                :key="project.id"
                class="px-4 py-3 text-left hover:bg-neon-cyan/10 hover:text-neon-cyan transition-colors flex items-center gap-2 text-gray-300"
                :class="{ 'text-neon-cyan bg-neon-cyan/5': selectedProjectId === project.id }"
                @click="selectProject(project.id)"
              >
                <div class="i-carbon-folder text-gray-500" />
                <span class="truncate">{{ project.path.split('/').pop() || project.path }}</span>
              </button>
            </div>
          </div>

          <!-- Divider -->
          <div class="w-px h-8 bg-white/10" />

          <!-- Date Range Filter -->
          <div class="flex items-center gap-2 bg-black/40 border border-white/10 rounded px-3 py-1.5 hover:border-neon-cyan/30 transition-colors group/date">
            <div class="i-carbon-calendar text-gray-500 group-hover/date:text-neon-cyan transition-colors" />
            
            <input
              v-model="startDate"
              type="date"
              class="bg-transparent border-none text-white focus:ring-0 p-0 w-[110px] uppercase tracking-wider text-center placeholder-gray-600"
              placeholder="START"
            />
            <span class="text-gray-600">→</span>
            <input
              v-model="endDate"
              type="date"
              class="bg-transparent border-none text-white focus:ring-0 p-0 w-[110px] uppercase tracking-wider text-center placeholder-gray-600"
              placeholder="END"
            />
          </div>

          <!-- Clear Filters Button -->
          <button
            v-if="selectedProjectId !== undefined || startDate || endDate"
            @click="clearFilters"
            class="ml-2 w-8 h-8 flex items-center justify-center rounded-full border border-neon-violet/30 text-neon-violet hover:bg-neon-violet hover:text-black hover:shadow-[0_0_15px_rgba(188,19,254,0.5)] transition-all"
            title="Clear Filters"
          >
            <div class="i-carbon-close" />
          </button>
        </div>
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
      <div v-if="!loading && results.length === 0 && sessionResults.length === 0" class="text-center py-20 opacity-50">
        <div class="font-mono text-neon-cyan mb-2">NO MATCHES FOUND</div>
        <div class="text-xs text-gray-600">TRY ADJUSTING QUERY PARAMETERS</div>
      </div>

      <!-- Content Search Results -->
      <div v-else-if="searchType === 'content'" class="space-y-4">
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
                <span :class="(('messageType' in result ? result.messageType : result.type)) === 'user' ? 'text-neon-cyan' : 'text-neon-violet'">
                  {{ (('messageType' in result ? result.messageType : result.type)) === 'user' ? 'USER' : 'SYSTEM' }}
                </span>
                <span class="text-gray-600">::</span>
                <span class="inline-flex items-center gap-1 px-2 py-0.5 rounded bg-white/5 border border-white/10 text-[10px] uppercase tracking-wide">
                  <span class="text-neon-cyan">{{ sourceLabel(result) }}</span>
                  <span v-if="channelLabel(result)" class="text-gray-500">/</span>
                  <span v-if="channelLabel(result)" class="text-gray-400">{{ channelLabel(result) }}</span>
                </span>
                <span class="text-gray-600">::</span>
                <span class="text-gray-500">{{ result.sessionId.slice(0, 8) }}</span>
                <template v-if="'projectName' in result && result.projectName">
                  <span class="text-gray-600">/</span>
                  <span class="text-neon-cyan/70">{{ result.projectName }}</span>
                </template>
              </div>
              <div class="flex items-center gap-2">
                <template v-if="'sources' in result">
                  <span v-if="result.sources.fts && result.sources.vector" class="text-neon-violet">FTS+VEC</span>
                  <span v-else-if="result.sources.fts" class="text-neon-cyan">FTS</span>
                  <span v-else-if="result.sources.vector" class="text-orange-400">VEC</span>
                </template>
                <span v-if="'timestamp' in result" class="text-gray-600">{{ formatTime(result.timestamp) }}</span>
              </div>
            </div>

            <div
              class="font-mono text-sm text-gray-300 leading-relaxed line-clamp-3"
              v-html="highlightKeyword(truncateContent(result.snippet || result.content), query)"
            />
          </div>
        </div>
      </div>

      <!-- Session ID Search Results -->
      <div v-else-if="searchType === 'sessionId'" class="space-y-4">
        <div
          v-for="session in sessionResults"
          :key="session.id"
          class="group relative bg-surface-100/50 border border-white/5 p-6 cursor-pointer hover:bg-surface-200/50 hover:border-neon-cyan/30 transition-all duration-300"
          @click="goToSession(session)"
        >
          <!-- Hover Glow -->
          <div class="absolute inset-0 bg-neon-cyan/5 opacity-0 group-hover:opacity-100 transition-opacity pointer-events-none" />

          <div class="relative z-10">
              <div class="flex items-center justify-between mb-3 font-mono text-xs">
              <div class="flex items-center gap-3">
                <span class="text-neon-cyan">SESSION</span>
                <span class="text-gray-600">::</span>
                <span class="inline-flex items-center px-2 py-0.5 rounded bg-white/5 border border-white/10 text-[10px] uppercase tracking-wide">
                  <span :style="{ color: (session.source || 'claude').toLowerCase() === 'claude' ? '#D97757' : '#00f3ff' }">
                    {{ session.source ? session.source.toUpperCase() : 'CLAUDE' }}
                  </span>
                </span>
                <span class="text-gray-600">::</span>
                <span class="text-white font-bold" v-html="highlightKeyword(session.id, query)"></span>
              </div>
              <div class="flex items-center gap-2">
                <span class="text-gray-600">{{ formatTime(session.createdAt) }}</span>
              </div>
            </div>

            <div class="font-mono text-sm text-gray-300 leading-relaxed">
              <div class="flex items-center gap-4">
                <span class="text-gray-500">Status:</span>
                <span class="text-neon-violet uppercase">{{ session.status }}</span>
                <span class="text-gray-600">|</span>
                <span class="text-gray-500">Messages:</span>
                <span class="text-neon-cyan">{{ session.messageCount }}</span>
                <span class="text-gray-600">|</span>
                <span class="text-gray-500">Updated:</span>
                <span class="text-gray-400">{{ formatTime(session.updatedAt) }}</span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>

  </div>
</template>
