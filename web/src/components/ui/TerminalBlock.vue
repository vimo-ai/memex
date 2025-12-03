<script setup lang="ts">
import { computed } from 'vue'
import { renderMarkdown } from '@/utils/markdown'

const props = defineProps<{
  content: string
  type: 'user' | 'assistant' | 'tool'
  timestamp: string
}>()

function formatTime(timestamp: string): string {
  return new Date(timestamp).toLocaleTimeString('en-US', {
    hour12: false,
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit'
  })
}

const renderedContent = computed(() => renderMarkdown(props.content))
</script>

<template>
  <div class="font-mono text-sm leading-relaxed mb-6 group max-w-[85%]">
    <!-- Header -->
    <div 
      class="flex items-center gap-2 mb-1 opacity-50 group-hover:opacity-100 transition-opacity select-none"
      :class="{ 'flex-row-reverse': type === 'user' }"
    >
      <span :class="type === 'user' ? 'text-neon-cyan' : type === 'assistant' ? 'text-neon-violet' : 'text-orange-400'">
        {{ type === 'user' ? 'root@user' : type === 'assistant' ? 'memex@ai' : 'tool@call' }}
      </span>
      <span class="text-gray-600">::</span>
      <span class="text-gray-500">[{{ formatTime(timestamp) }}]</span>
      <span class="text-gray-600">{{ type === 'user' ? '<<' : '>>' }}</span>
    </div>

    <!-- Content -->
    <div 
      class="markdown-content break-words py-2"
      :class="[
        type === 'user' 
          ? 'text-white border-r-2 border-neon-cyan/50 bg-neon-cyan/5 pr-4 pl-2 text-right' 
          : 'text-gray-300 border-l-2 border-neon-violet/50 pl-4'
      ]"
    >
      <!-- 
        User content: We want the BLOCK to be right-aligned (handled by parent flex),
        but the TEXT inside should be left-aligned for readability (especially code).
        However, for short chat messages, right-align text looks better?
        User complained about "margin inside block".
        Let's just use text-left for everything to be safe and clean, 
        but the container itself has the border on the right.
      -->
      <div class="text-left" v-html="renderedContent" />
    </div>
  </div>
</template>
