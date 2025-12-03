<script setup lang="ts">
import { ref, onMounted, computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { getProject, getProjectSessions, type Project, type Session } from '@/api'
import GlitchText from '@/components/ui/GlitchText.vue'

const route = useRoute()
const router = useRouter()

// State
const project = ref<Project | null>(null)
const sessions = ref<Session[]>([])
const loading = ref(true)
const error = ref<string | null>(null)

const projectId = Number(route.params.id)

// Computed
const totalMessages = computed(() => {
  return sessions.value.reduce((acc, session) => acc + session.messageCount, 0)
})

const lastActive = computed(() => {
  if (sessions.value.length === 0) return 'NEVER'
  const lastSession = sessions.value[0]
  return lastSession ? formatTime(lastSession.updatedAt) : 'NEVER'
})

// Actions
async function loadData() {
  loading.value = true
  error.value = null

  try {
    const [projectData, sessionsData] = await Promise.all([
      getProject(projectId),
      getProjectSessions(projectId),
    ])
    project.value = projectData
    sessions.value = sessionsData
  } catch (e) {
    console.error('Failed to load data:', e)
    error.value = 'DATA CORRUPTION DETECTED'
  } finally {
    loading.value = false
  }
}

function goToSession(session: Session) {
  router.push(`/sessions/${session.id}`)
}

function getProjectName(path: string): string {
  const parts = path.split('/')
  return parts[parts.length - 1] || path
}

function formatTime(timestamp: string): string {
  return new Date(timestamp).toLocaleString('en-US', {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit'
  })
}

function shortUuid(uuid: string): string {
  return uuid.slice(0, 8)
}

function goBack() {
  router.push('/projects')
}

onMounted(() => {
  loadData()
})
</script>

<template>
  <div class="h-full flex flex-col px-8 pt-8">
    <!-- Loading State -->
    <div v-if="loading" class="flex justify-center py-20">
      <div class="i-carbon-circle-dash animate-spin text-4xl text-neon-cyan" />
    </div>

    <!-- Error State -->
    <div v-else-if="error" class="text-center py-20 text-red-500 font-mono">
      {{ error }}
    </div>

    <template v-else-if="project">
      <!-- Header -->
      <div class="flex items-start justify-between mb-8 border-b border-white/5 pb-6">
        <div>
          <button 
            class="flex items-center gap-2 text-xs font-mono text-gray-400 hover:text-neon-cyan mb-4 transition-colors"
            @click="goBack"
          >
            <div class="i-carbon-arrow-left" />
            RETURN TO ARCHIVES
          </button>
          
          <GlitchText :text="getProjectName(project.path)" class="text-4xl font-bold text-white mb-2" />
          <div class="text-xs font-mono text-gray-500 tracking-widest truncate max-w-2xl">
            PATH: {{ project.path }}
          </div>
        </div>

        <!-- Stats -->
        <div class="flex gap-8 text-right">
          <div>
            <div class="text-2xl font-display text-white">{{ sessions.length }}</div>
            <div class="text-xs font-mono text-gray-500">SESSIONS</div>
          </div>
          <div>
            <div class="text-2xl font-display text-white">{{ totalMessages }}</div>
            <div class="text-xs font-mono text-gray-500">MESSAGES</div>
          </div>
          <div>
            <div class="text-2xl font-display text-white">{{ lastActive }}</div>
            <div class="text-xs font-mono text-gray-500">LAST SYNC</div>
          </div>
        </div>
      </div>

      <!-- Timeline Stream -->
      <div class="flex-1 overflow-y-auto pb-32 pr-4 scroll-smooth relative">
        <!-- Vertical Line -->
        <div class="absolute left-4 top-0 bottom-0 w-px bg-white/10" />

        <div class="space-y-8 pl-12 relative">
          <div v-if="sessions.length === 0" class="text-gray-500 font-mono py-8">
            NO DATA STREAMS FOUND
          </div>

          <div
            v-for="session in sessions"
            :key="session.id"
            class="group relative"
          >
            <!-- Timeline Node -->
            <div class="absolute -left-[39px] top-1.5 w-3 h-3 rounded-full bg-surface-300 border border-white/20 group-hover:bg-neon-cyan group-hover:shadow-[0_0_10px_rgba(0,243,255,0.8)] transition-all duration-300 z-10" />

            <!-- Card -->
            <div 
              class="bg-surface-100/30 border border-white/5 p-4 cursor-pointer hover:bg-surface-200/50 hover:border-neon-cyan/30 hover:translate-x-2 transition-all duration-300"
              @click="goToSession(session)"
            >
              <div class="flex items-center justify-between mb-2">
              <div class="flex items-center gap-3">
                <span class="font-mono text-neon-cyan text-sm">ID: {{ shortUuid(session.id) }}</span>
                <span class="inline-flex items-center px-2 py-0.5 rounded bg-white/5 border border-white/10 text-[10px] uppercase tracking-wide">
                  <span :style="{ color: (session.source || 'claude').toLowerCase() === 'claude' ? '#D97757' : '#00f3ff' }">
                    {{ (session.source || 'claude').toUpperCase() }}
                  </span>
                </span>
                <span
                  class="text-xs px-1.5 py-0.5 rounded border"
                  :class="session.messageCount > 0
                    ? 'border-neon-cyan/30 text-neon-cyan bg-neon-cyan/5'
                    : 'border-gray-700 text-gray-500'"
                  >
                    {{ session.messageCount > 0 ? 'ACTIVE' : 'EMPTY' }}
                  </span>
                </div>
                <span class="text-xs font-mono text-gray-500">{{ formatTime(session.updatedAt) }}</span>
              </div>

              <div class="flex items-center gap-4 text-xs text-gray-400 font-mono">
                <span class="flex items-center gap-1">
                  <div class="i-carbon-document" />
                  {{ session.messageCount }} MSGS
                </span>
                <span class="flex items-center gap-1">
                  <div class="i-carbon-time" />
                  {{ formatTime(session.createdAt) }}
                </span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </template>
  </div>
</template>
