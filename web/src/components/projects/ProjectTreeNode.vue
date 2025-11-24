<script setup lang="ts">
import { computed } from 'vue'
import { useRouter } from 'vue-router'
import type { TreeNode } from '@/utils/pathUtils'
import { useTreeState } from '@/composables/useTreeState'

const props = defineProps<{
  node: TreeNode
  depth?: number
}>()

const router = useRouter()
const { isExpanded, toggle } = useTreeState()

const currentDepth = computed(() => props.depth || 0)

// Sort children: Folders first, then Projects, alphabetical
const sortedChildren = computed(() => {
  if (!props.node.children) return []
  return Array.from(props.node.children.values()).sort((a, b) => {
    if (a.type !== b.type) {
      return a.type === 'folder' ? -1 : 1
    }
    return a.name.localeCompare(b.name)
  })
})

function handleClick() {
  if (props.node.type === 'folder') {
    toggle(props.node.fullPath)
  } else if (props.node.project) {
    router.push(`/projects/${props.node.project.id}`)
  }
}

function formatTime(timestamp: string): string {
  return new Date(timestamp).toLocaleDateString('en-US', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit'
  })
}
</script>

<template>
  <div class="select-none">
    <!-- Node Card -->
    <div 
      class="group relative bg-surface-100/30 border border-white/5 p-4 cursor-pointer hover:bg-surface-200/50 hover:border-neon-cyan/50 transition-all duration-300 mb-2 rounded-lg flex items-center gap-4"
      :class="{ 'border-neon-cyan/30 bg-surface-200/30': isExpanded(node.fullPath) }"
      :style="{ marginLeft: `${currentDepth * 24}px` }"
      @click.stop="handleClick"
    >
      <!-- Hover Glow -->
      <div class="absolute inset-0 bg-neon-cyan/5 opacity-0 group-hover:opacity-100 transition-opacity pointer-events-none rounded-lg" />

      <!-- Icon -->
      <div 
        class="transition-colors flex-shrink-0" 
        :class="node.type === 'folder' ? 'text-neon-violet' : 'text-gray-600 group-hover:text-neon-cyan'"
      >
        <div v-if="node.type === 'folder'" class="text-2xl" :class="isExpanded(node.fullPath) ? 'i-carbon-folder-open' : 'i-carbon-folder'" />
        <div v-else class="i-carbon-cube text-xl" />
      </div>

      <!-- Info -->
      <div class="flex-1 min-w-0">
        <div class="flex items-center gap-3">
          <h3 class="text-base font-bold text-gray-200 group-hover:text-white truncate font-display tracking-wide">
            {{ node.name }}
          </h3>
          
          <!-- Badges -->
          <span v-if="node.type === 'folder'" class="text-[10px] font-mono px-1.5 py-0.5 rounded bg-neon-violet/10 text-neon-violet border border-neon-violet/20">
            {{ node.count }} ITEMS
          </span>
          <span v-if="node.project" class="text-[10px] font-mono px-1.5 py-0.5 rounded bg-neon-cyan/10 text-neon-cyan border border-neon-cyan/20">
            PROJECT
          </span>
        </div>
        
        <div v-if="node.project" class="text-xs text-gray-600 font-mono mt-1 flex items-center justify-between gap-4">
          <div class="flex gap-4">
            <span>{{ node.project.sessionCount }} SESSIONS</span>
            <span>UPDATED: {{ formatTime(node.project.updatedAt) }}</span>
          </div>
          
          <!-- Enter Button for Hybrid Nodes (Folders that are also Projects) -->
          <button 
            v-if="node.type === 'folder'"
            class="px-2 py-0.5 rounded border border-neon-cyan/30 text-neon-cyan hover:bg-neon-cyan/10 transition-colors text-[10px] uppercase tracking-wider"
            @click.stop="router.push(`/projects/${node.project.id}`)"
          >
            Enter Project
          </button>
        </div>
      </div>

      <!-- Chevron for Folders -->
      <div v-if="node.type === 'folder'" class="text-gray-600 transition-transform duration-300" :class="{ 'rotate-90': isExpanded(node.fullPath) }">
        <div class="i-carbon-chevron-right" />
      </div>
    </div>

    <!-- Children (Recursive) -->
    <div v-if="isExpanded(node.fullPath) && node.type === 'folder'" class="flex flex-col">
      <ProjectTreeNode
        v-for="child in sortedChildren"
        :key="child.fullPath"
        :node="child"
        :depth="currentDepth + 1"
      />
    </div>
  </div>
</template>
