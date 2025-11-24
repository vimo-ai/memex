<script setup lang="ts">
import { useRoute } from 'vue-router'
import { computed } from 'vue'

const route = useRoute()

const breadcrumbs = computed(() => {
  const path = route.path
  if (path === '/') return ['Projects']
  if (path.startsWith('/project/')) return ['Projects', 'Detail']
  if (path.startsWith('/session/')) return ['Projects', 'Session']
  if (path === '/search') return ['Search']
  return []
})
</script>

<template>
  <header class="h-16 px-8 flex items-center justify-between border-b border-white/5 bg-slate-950/50 backdrop-blur-sm sticky top-0 z-10">
    <div class="flex items-center gap-2 text-sm">
      <template v-for="(crumb, index) in breadcrumbs" :key="crumb">
        <span :class="index === breadcrumbs.length - 1 ? 'text-slate-200 font-medium' : 'text-slate-500'">
          {{ crumb }}
        </span>
        <div v-if="index < breadcrumbs.length - 1" class="i-carbon-chevron-right text-slate-600 text-xs" />
      </template>
    </div>

    <div class="flex items-center gap-4">
      <button class="w-8 h-8 rounded-full hover:bg-white/5 flex items-center justify-center text-slate-400 transition-colors">
        <div class="i-carbon-notification" />
      </button>
      <button class="w-8 h-8 rounded-full hover:bg-white/5 flex items-center justify-center text-slate-400 transition-colors">
        <div class="i-carbon-settings" />
      </button>
    </div>
  </header>
</template>
