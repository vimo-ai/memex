<script setup lang="ts">
import { useRoute } from 'vue-router'

const route = useRoute()

const links = [
  { name: 'Home', path: '/', icon: 'i-carbon-home' },
  { name: 'Archives', path: '/projects', icon: 'i-carbon-data-base' },
]
</script>

<template>
  <nav class="fixed bottom-8 left-1/2 -translate-x-1/2 z-50">
    <div class="flex items-center gap-2 px-4 py-3 bg-surface-100/80 backdrop-blur-xl border border-white/10 rounded-2xl shadow-2xl shadow-black/50">
      <router-link
        v-for="link in links"
        :key="link.path"
        :to="link.path"
        class="relative group p-3 rounded-xl transition-all duration-300 hover:bg-white/5"
        :class="route.path === link.path ? 'text-neon-cyan' : 'text-gray-400'"
      >
        <div :class="link.icon" class="text-2xl transition-transform duration-300 group-hover:-translate-y-1 group-hover:scale-110" />
        
        <!-- Tooltip -->
        <div class="absolute -top-10 left-1/2 -translate-x-1/2 px-2 py-1 bg-surface-200 border border-white/10 rounded text-xs font-mono text-white opacity-0 group-hover:opacity-100 transition-opacity pointer-events-none whitespace-nowrap">
          {{ link.name }}
        </div>

        <!-- Active Indicator -->
        <div 
          v-if="route.path === link.path"
          class="absolute -bottom-1 left-1/2 -translate-x-1/2 w-1 h-1 rounded-full bg-neon-cyan shadow-[0_0_5px_rgba(0,243,255,0.8)]"
        />
      </router-link>
    </div>
  </nav>
</template>
