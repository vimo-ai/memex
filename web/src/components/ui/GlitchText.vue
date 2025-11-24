<script setup lang="ts">
defineProps<{
  text: string
}>()
</script>

<template>
  <div class="relative group inline-block">
    <span class="relative z-10 font-display tracking-widest uppercase">{{ text }}</span>
    <span class="absolute top-0 left-0 -z-10 w-full h-full text-neon-cyan opacity-0 group-hover:opacity-70 group-hover:animate-[glitch_0.3s_infinite] translate-x-[2px]">{{ text }}</span>
    <span class="absolute top-0 left-0 -z-10 w-full h-full text-neon-violet opacity-0 group-hover:opacity-70 group-hover:animate-[glitch_0.3s_infinite_reverse] -translate-x-[2px]">{{ text }}</span>
  </div>
</template>
