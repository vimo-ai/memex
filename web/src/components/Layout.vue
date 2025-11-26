<script setup lang="ts">
import TopBar from './layout/TopBar.vue'
import Dock from './layout/Dock.vue'
</script>

<template>
  <div class="min-h-screen bg-void text-gray-300 font-sans selection:bg-neon-cyan/20 selection:text-white overflow-hidden relative">
    <!-- Background Grid Effect -->
    <div class="absolute inset-0 bg-[linear-gradient(rgba(255,255,255,0.02)_1px,transparent_1px),linear-gradient(90deg,rgba(255,255,255,0.02)_1px,transparent_1px)] bg-[size:50px_50px] pointer-events-none" />
    
    <TopBar />
    
    <main class="h-screen pt-10 pb-32 overflow-hidden">
      <router-view v-slot="{ Component }">
        <transition name="fade" mode="out-in">
          <component :is="Component" />
        </transition>
      </router-view>
    </main>

    <Dock />
  </div>
</template>
