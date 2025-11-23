<script setup lang="ts">
import { useRoute, useRouter } from 'vue-router'

const route = useRoute()
const router = useRouter()

// 判断是否在首页
const isHome = () => route.path === '/'

// 导航到搜索页
function goToSearch() {
  router.push('/')
}

// 导航到项目列表
function goToProjects() {
  router.push('/projects')
}
</script>

<template>
  <div class="min-h-screen bg-gray-900 text-gray-100">
    <!-- 顶部导航栏 -->
    <header class="sticky top-0 z-50 bg-gray-800 border-b border-gray-700">
      <nav class="max-w-7xl mx-auto px-4 h-14 flex items-center justify-between">
        <!-- Logo -->
        <div
          class="flex items-center gap-2 cursor-pointer hover:opacity-80 transition-opacity"
          @click="goToSearch"
        >
          <div class="i-carbon-data-base text-2xl text-blue-400" />
          <span class="text-lg font-semibold">Memex</span>
        </div>

        <!-- 导航链接 -->
        <div class="flex items-center gap-4">
          <!-- 搜索入口（非首页显示） -->
          <button
            v-if="!isHome()"
            class="flex items-center gap-1 px-3 py-1.5 rounded-lg bg-gray-700 hover:bg-gray-600 transition-colors text-sm"
            @click="goToSearch"
          >
            <div class="i-carbon-search" />
            <span>搜索</span>
          </button>

          <!-- 项目列表链接 -->
          <button
            class="flex items-center gap-1 px-3 py-1.5 rounded-lg hover:bg-gray-700 transition-colors text-sm"
            :class="{ 'bg-gray-700': route.path.startsWith('/projects') }"
            @click="goToProjects"
          >
            <div class="i-carbon-folder" />
            <span>项目</span>
          </button>
        </div>
      </nav>
    </header>

    <!-- 主内容区域 -->
    <main class="max-w-7xl mx-auto px-4 py-6">
      <slot />
    </main>
  </div>
</template>
