<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue'

const time = ref('')
const isOnline = ref(navigator.onLine)

function updateTime() {
  const now = new Date()
  time.value = now.toLocaleTimeString('en-US', { 
    hour12: false, 
    hour: '2-digit', 
    minute: '2-digit',
    second: '2-digit'
  })
}

function updateOnlineStatus() {
  isOnline.value = navigator.onLine
}

let timer: ReturnType<typeof setInterval>

onMounted(() => {
  updateTime()
  timer = setInterval(updateTime, 1000)
  window.addEventListener('online', updateOnlineStatus)
  window.addEventListener('offline', updateOnlineStatus)
})

onUnmounted(() => {
  clearInterval(timer)
  window.removeEventListener('online', updateOnlineStatus)
  window.removeEventListener('offline', updateOnlineStatus)
})
</script>

<template>
  <header class="fixed top-0 left-0 right-0 h-10 bg-void/80 backdrop-blur-sm border-b border-white/5 z-50 flex items-center justify-between px-6 select-none">
    <!-- Left: System Status -->
    <div class="flex items-center gap-4 text-xs font-mono text-gray-500">
      <div class="flex items-center gap-2">
        <div class="w-1.5 h-1.5 rounded-full" :class="isOnline ? 'bg-neon-cyan shadow-[0_0_5px_rgba(0,243,255,0.5)]' : 'bg-red-500'" />
        <span>{{ isOnline ? 'SYSTEM ONLINE' : 'OFFLINE' }}</span>
      </div>
      <span>MEMEX.OS v2.0</span>
    </div>

    <!-- Center: Title (Hidden on small screens) -->
    <div class="hidden md:block absolute left-1/2 -translate-x-1/2">
      <div class="text-xs font-display tracking-[0.2em] text-gray-600">NEURAL INTERFACE</div>
    </div>

    <!-- Right: Time -->
    <div class="text-xs font-mono text-neon-cyan">
      {{ time }}
    </div>
  </header>
</template>
