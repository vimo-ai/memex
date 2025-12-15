<script setup lang="ts">
import { useRouter } from 'vue-router'
import GlitchText from '@/components/ui/GlitchText.vue'

const router = useRouter()

function goHome() {
  router.push('/search')
}
</script>

<template>
  <div class="h-full flex flex-col items-center justify-center relative">
    <!-- Background Effect -->
    <div class="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 w-[400px] h-[400px] bg-red-500/10 rounded-full blur-[100px] pointer-events-none" />

    <div class="z-10 text-center">
      <!-- Error Code -->
      <div class="mb-8">
        <GlitchText text="404" class="text-8xl font-bold text-red-500" />
      </div>

      <!-- Message -->
      <div class="font-mono text-gray-400 mb-2 tracking-widest">
        > ERROR: ROUTE_NOT_FOUND
      </div>
      <div class="font-mono text-gray-600 text-sm mb-12">
        > THE REQUESTED PATH DOES NOT EXIST IN THE NEURAL NETWORK
      </div>

      <!-- Action -->
      <button
        @click="goHome"
        class="px-8 py-3 border border-neon-cyan text-neon-cyan font-mono uppercase tracking-widest hover:bg-neon-cyan hover:text-black transition-all duration-300"
      >
        RETURN TO ORACLE
      </button>
    </div>
  </div>
</template>
