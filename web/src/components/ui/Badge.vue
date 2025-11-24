<script setup lang="ts">
interface Props {
  variant?: 'default' | 'success' | 'warning' | 'error' | 'info'
}

withDefaults(defineProps<Props>(), {
  variant: 'default',
})

const variants = {
  default: 'bg-slate-800 text-slate-300 border-slate-700',
  success: 'bg-emerald-500/10 text-emerald-400 border-emerald-500/20 shadow-emerald-500/10',
  warning: 'bg-amber-500/10 text-amber-400 border-amber-500/20 shadow-amber-500/10',
  error: 'bg-red-500/10 text-red-400 border-red-500/20 shadow-red-500/10',
  info: 'bg-blue-500/10 text-blue-400 border-blue-500/20 shadow-blue-500/10',
}
</script>

<template>
  <span 
    class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium border shadow-[0_0_10px_rgba(0,0,0,0)]"
    :class="variants[variant]"
  >
    <slot />
  </span>
</template>
