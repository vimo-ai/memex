<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue'
import { useRoute } from 'vue-router'

const route = useRoute()

const links = [
  { name: 'Search', path: '/search', icon: 'i-carbon-search' },
  { name: 'Ask AI', path: '/ask', icon: 'i-carbon-ibm-watson-discovery' },
  { name: 'Archives', path: '/projects', icon: 'i-carbon-data-base' },
]

// 判断路由是否匹配（支持子路由高亮）
function isActive(linkPath: string): boolean {
  if (linkPath === '/search') {
    return route.path === '/search' || route.path === '/'
  }
  return route.path.startsWith(linkPath)
}

// 拖拽相关
const STORAGE_KEY = 'memex-dock-position'
const dockRef = ref<HTMLElement | null>(null)
const isDragging = ref(false)
const position = ref({ x: 0, y: 0 })
const dragOffset = ref({ x: 0, y: 0 })

// 从 localStorage 加载位置
function loadPosition() {
  const saved = localStorage.getItem(STORAGE_KEY)
  if (saved) {
    try {
      const pos = JSON.parse(saved)
      position.value = pos
    } catch {
      // 使用默认位置（底部居中）
      resetToDefault()
    }
  } else {
    resetToDefault()
  }
}

// 重置到默认位置（底部居中）
function resetToDefault() {
  position.value = {
    x: window.innerWidth / 2,
    y: window.innerHeight - 60
  }
}

// 保存位置到 localStorage
function savePosition() {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(position.value))
}

// 拖拽开始
function onMouseDown(e: MouseEvent) {
  if (!dockRef.value) return
  isDragging.value = true
  const rect = dockRef.value.getBoundingClientRect()
  dragOffset.value = {
    x: e.clientX - (rect.left + rect.width / 2),
    y: e.clientY - (rect.top + rect.height / 2)
  }
  e.preventDefault()
}

// 拖拽中
function onMouseMove(e: MouseEvent) {
  if (!isDragging.value || !dockRef.value) return
  const rect = dockRef.value.getBoundingClientRect()
  const halfWidth = rect.width / 2
  const halfHeight = rect.height / 2

  // 限制在窗口范围内
  let newX = e.clientX - dragOffset.value.x
  let newY = e.clientY - dragOffset.value.y

  newX = Math.max(halfWidth, Math.min(window.innerWidth - halfWidth, newX))
  newY = Math.max(halfHeight, Math.min(window.innerHeight - halfHeight, newY))

  position.value = { x: newX, y: newY }
}

// 拖拽结束
function onMouseUp() {
  if (isDragging.value) {
    isDragging.value = false
    savePosition()
  }
}

onMounted(() => {
  loadPosition()
  document.addEventListener('mousemove', onMouseMove)
  document.addEventListener('mouseup', onMouseUp)
})

onUnmounted(() => {
  document.removeEventListener('mousemove', onMouseMove)
  document.removeEventListener('mouseup', onMouseUp)
})
</script>

<template>
  <nav
    ref="dockRef"
    class="fixed z-50 select-none"
    :style="{
      left: `${position.x}px`,
      top: `${position.y}px`,
      transform: 'translate(-50%, -50%)'
    }"
  >
    <div
      class="flex items-center gap-1 px-2 py-1.5 bg-surface-100/80 backdrop-blur-xl border border-white/10 rounded-xl shadow-2xl shadow-black/50"
      :class="{ 'cursor-grabbing': isDragging, 'cursor-grab': !isDragging }"
      @mousedown="onMouseDown"
    >
      <router-link
        v-for="link in links"
        :key="link.path"
        :to="link.path"
        class="relative group p-2 rounded-lg transition-all duration-300 hover:bg-white/5"
        :class="isActive(link.path) ? 'text-neon-cyan' : 'text-gray-400'"
        @click.stop
      >
        <div :class="link.icon" class="text-base transition-transform duration-300 group-hover:-translate-y-0.5 group-hover:scale-110" />

        <!-- Tooltip -->
        <div class="absolute -top-8 left-1/2 -translate-x-1/2 px-2 py-1 bg-surface-200 border border-white/10 rounded text-xs font-mono text-white opacity-0 group-hover:opacity-100 transition-opacity pointer-events-none whitespace-nowrap">
          {{ link.name }}
        </div>

        <!-- Active Indicator -->
        <div
          v-if="isActive(link.path)"
          class="absolute -bottom-0.5 left-1/2 -translate-x-1/2 w-1 h-1 rounded-full bg-neon-cyan shadow-[0_0_5px_rgba(0,243,255,0.8)]"
        />
      </router-link>
    </div>
  </nav>
</template>
