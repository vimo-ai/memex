<script setup lang="ts">
interface Props {
  hover?: boolean
  padding?: string
}

withDefaults(defineProps<Props>(), {
  hover: true,
  padding: 'p-6',
})
</script>

<template>
  <div 
    class="bg-surface rounded-xl border border-white/5 transition-all duration-300"
    :class="[
      padding,
      hover ? 'hover:bg-surface-hover hover:-translate-y-1 hover:shadow-xl hover:shadow-primary-500/5' : ''
    ]"
  >
    <slot />
  </div>
</template>
