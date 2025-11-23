import { defineConfig, presetUno, presetIcons } from 'unocss'

export default defineConfig({
  presets: [
    presetUno(),
    presetIcons({
      scale: 1.2,
      cdn: 'https://esm.sh/',
    }),
  ],
  // 深色主题快捷方式
  shortcuts: {
    'bg-base': 'bg-gray-900',
    'text-base': 'text-gray-100',
    'border-base': 'border-gray-700',
  },
})
