<script setup lang="ts">
import { useRoute } from 'vue-router'

const route = useRoute()

const links = [
  { name: 'Oracle', path: '/search', icon: 'i-carbon-search' },
  { name: 'Projects', path: '/projects', icon: 'i-carbon-ibm-cloud-projects' },
  { name: 'Ask AI', path: '/ask', icon: 'i-carbon-ibm-watson-discovery' },
]

// 判断路由是否匹配（支持子路由高亮）
function isActive(linkPath: string): boolean {
  if (linkPath === '/search') {
    return route.path === '/search' || route.path === '/'
  }
  return route.path.startsWith(linkPath)
}
</script>

<template>
  <aside class="w-64 h-full bg-slate-950/50 backdrop-blur-xl border-r border-white/5 flex flex-col">
    <div class="p-6 flex items-center gap-3">
      <div class="w-8 h-8 rounded-lg bg-gradient-to-br from-primary-500 to-indigo-600 flex items-center justify-center shadow-lg shadow-primary-500/20">
        <div class="i-carbon-code text-white text-lg" />
      </div>
      <span class="text-lg font-bold bg-clip-text text-transparent bg-gradient-to-r from-white to-slate-400">
        Memex
      </span>
    </div>

    <nav class="flex-1 px-4 py-4 space-y-1">
      <router-link
        v-for="link in links"
        :key="link.path"
        :to="link.path"
        class="flex items-center gap-3 px-4 py-3 rounded-xl transition-all duration-200 group"
        :class="isActive(link.path)
          ? 'bg-primary-500/10 text-primary-400'
          : 'text-slate-400 hover:text-slate-200 hover:bg-white/5'"
      >
        <div :class="link.icon" class="text-xl transition-transform group-hover:scale-110" />
        <span class="font-medium">{{ link.name }}</span>

        <div
          v-if="isActive(link.path)"
          class="ml-auto w-1.5 h-1.5 rounded-full bg-primary-400 shadow-[0_0_8px_currentColor]"
        />
      </router-link>
    </nav>

    <div class="p-4 border-t border-white/5">
      <div class="flex items-center gap-3 px-4 py-3 rounded-xl bg-white/5 border border-white/5">
        <div class="w-8 h-8 rounded-full bg-slate-800 flex items-center justify-center">
          <div class="i-carbon-user text-slate-400" />
        </div>
        <div class="flex-1 min-w-0">
          <div class="text-sm font-medium text-slate-200 truncate">User</div>
          <div class="text-xs text-slate-500 truncate">Pro Plan</div>
        </div>
      </div>
    </div>
  </aside>
</template>
