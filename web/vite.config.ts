import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import UnoCSS from 'unocss/vite'
import { resolve } from 'path'

// https://vite.dev/config/
export default defineConfig({
  plugins: [
    vue(),
    UnoCSS(),
  ],
  resolve: {
    alias: {
      '@': resolve(__dirname, 'src'),
    },
  },
  server: {
    port: 10086,
    proxy: {
      // 代理 /api 到后端服务
      '/api': {
        target: 'http://localhost:10013',
        changeOrigin: true,
      },
    },
  },
})
