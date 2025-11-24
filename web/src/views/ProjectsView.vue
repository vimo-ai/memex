<script setup lang="ts">
import { ref, onMounted, computed } from 'vue'
import { getProjects, type Project } from '@/api'
import { buildProjectTree, type TreeNode } from '@/utils/pathUtils'
import GlitchText from '@/components/ui/GlitchText.vue'
import ProjectTreeNode from '@/components/projects/ProjectTreeNode.vue'

// State
const projects = ref<Project[]>([])
const treeRoot = ref<TreeNode | null>(null)
const loading = ref(true)
const error = ref<string | null>(null)
const searchQuery = ref('')

// Computed
const rootChildren = computed(() => {
  if (!treeRoot.value) return []
  
  // If searching, flatten results
  if (searchQuery.value) {
    const query = searchQuery.value.toLowerCase()
    return projects.value
      .filter(p => p.path.toLowerCase().includes(query))
      .map(p => ({
        name: getProjectName(p.path),
        fullPath: p.path,
        type: 'project',
        project: p,
        count: 0,
        children: new Map()
      } as TreeNode))
  }

  // Return root children sorted
  if (!treeRoot.value.children) return []
  return Array.from(treeRoot.value.children.values()).sort((a, b) => {
    if (a.type !== b.type) {
      return a.type === 'folder' ? -1 : 1
    }
    return a.name.localeCompare(b.name)
  })
})

// Actions
async function loadProjects() {
  loading.value = true
  error.value = null

  try {
    projects.value = await getProjects()
    treeRoot.value = buildProjectTree(projects.value)
  } catch (e) {
    console.error('Failed to load projects:', e)
    error.value = 'CONNECTION FAILURE'
  } finally {
    loading.value = false
  }
}

function getProjectName(path: string): string {
  const parts = path.split('/')
  return parts[parts.length - 1] || path
}

onMounted(() => {
  loadProjects()
})
</script>

<template>
  <div class="h-full flex flex-col px-8 pt-8">
    <!-- Header -->
    <div class="flex flex-wrap items-end justify-between mb-8 border-b border-white/5 pb-6 gap-4">
      <div class="max-w-full flex-1 min-w-[200px]">
        <div class="relative group inline-block text-4xl font-bold text-white mb-2 max-w-full overflow-hidden text-ellipsis">
          <GlitchText text="ARCHIVES" />
        </div>
        <div class="text-xs font-mono text-neon-cyan tracking-widest truncate">
          {{ projects.length }} DATA NODES DETECTED
        </div>
      </div>

      <!-- Filter -->
      <div class="relative group w-full md:w-64 max-w-full flex-shrink-0">
        <div class="absolute inset-0 bg-neon-cyan/20 blur-md opacity-0 group-hover:opacity-100 transition-opacity"></div>
        <input 
          v-model="searchQuery"
          type="text" 
          placeholder="SEARCH NODES..."
          class="relative w-full bg-surface-100/80 border border-white/10 px-4 py-2 text-sm font-mono text-white focus:border-neon-cyan/50 focus:outline-none transition-colors"
        >
      </div>
    </div>

    <!-- Tree List -->
    <div class="flex-1 overflow-y-auto pb-32 pr-2 scroll-smooth">
      <div v-if="loading" class="flex justify-center py-20">
        <div class="i-carbon-circle-dash animate-spin text-4xl text-neon-cyan" />
      </div>

      <div v-else-if="error" class="text-center py-20 text-red-500 font-mono">
        {{ error }}
      </div>

      <div v-else-if="rootChildren.length === 0" class="text-center py-20 text-gray-600 font-mono">
        EMPTY SECTOR
      </div>

      <div v-else class="flex flex-col gap-2">
        <ProjectTreeNode
          v-for="node in rootChildren"
          :key="node.fullPath"
          :node="node"
        />
      </div>
    </div>
  </div>
</template>
