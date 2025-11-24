<script setup lang="ts">
import { computed } from 'vue'

interface Props {
  variant?: 'primary' | 'secondary' | 'ghost' | 'danger'
  size?: 'sm' | 'md' | 'lg'
  loading?: boolean
  disabled?: boolean
  icon?: string
}

const props = withDefaults(defineProps<Props>(), {
  variant: 'primary',
  size: 'md',
  loading: false,
  disabled: false,
})

const classes = computed(() => {
  const variants = {
    primary: 'btn-primary',
    secondary: 'btn-secondary',
    ghost: 'btn-ghost',
    danger: 'btn-danger',
  }
  
  const sizes = {
    sm: 'text-sm px-3 py-1.5',
    md: 'text-base px-4 py-2',
    lg: 'text-lg px-6 py-3',
  }

  return [
    'btn',
    variants[props.variant],
    sizes[props.size],
    props.loading ? 'cursor-wait opacity-75' : '',
  ]
})
</script>

<template>
  <button :class="classes" :disabled="disabled || loading">
    <div v-if="loading" class="i-carbon-circle-dash animate-spin text-xl" />
    <div v-else-if="icon" :class="icon" class="text-xl" />
    <slot />
  </button>
</template>
