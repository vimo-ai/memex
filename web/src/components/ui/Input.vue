<script setup lang="ts">
interface Props {
  modelValue: string | number
  type?: string
  placeholder?: string
  label?: string
  icon?: string
}

defineProps<Props>()
defineEmits<{
  (e: 'update:modelValue', value: string | number): void
}>()
</script>

<template>
  <div class="flex flex-col gap-1.5">
    <label v-if="label" class="text-sm font-medium text-muted ml-1">{{ label }}</label>
    <div class="relative group">
      <div v-if="icon" :class="icon" class="absolute left-3 top-1/2 -translate-y-1/2 text-muted group-focus-within:text-primary-400 transition-colors" />
      <input
        :value="modelValue"
        :type="type || 'text'"
        :placeholder="placeholder"
        class="input w-full"
        :class="{ 'pl-10': icon }"
        @input="$emit('update:modelValue', ($event.target as HTMLInputElement).value)"
      >
    </div>
  </div>
</template>
