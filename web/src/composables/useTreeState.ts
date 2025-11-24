import { ref, watch } from 'vue'

const STORAGE_KEY = 'memex:tree:expanded'

// Global state to share across components
const expandedPaths = ref<Set<string>>(new Set())
const initialized = ref(false)

export function useTreeState() {
  // Initialize from localStorage once
  if (!initialized.value) {
    try {
      const stored = localStorage.getItem(STORAGE_KEY)
      if (stored) {
        const paths = JSON.parse(stored)
        if (Array.isArray(paths)) {
          expandedPaths.value = new Set(paths)
        }
      }
    } catch (e) {
      console.error('Failed to load tree state:', e)
    }
    initialized.value = true
  }

  // Watch for changes and save
  watch(expandedPaths, (newPaths) => {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(Array.from(newPaths)))
    } catch (e) {
      console.error('Failed to save tree state:', e)
    }
  }, { deep: true })

  function isExpanded(path: string): boolean {
    return expandedPaths.value.has(path)
  }

  function toggle(path: string) {
    if (expandedPaths.value.has(path)) {
      expandedPaths.value.delete(path)
    } else {
      expandedPaths.value.add(path)
    }
  }

  function setExpanded(path: string, expanded: boolean) {
    if (expanded) {
      expandedPaths.value.add(path)
    } else {
      expandedPaths.value.delete(path)
    }
  }

  return {
    isExpanded,
    toggle,
    setExpanded
  }
}
