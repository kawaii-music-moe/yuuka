import { fileURLToPath, URL } from 'node:url'
import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

export default defineConfig({
  base: '/',
  plugins: [vue()],
  resolve: { alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) } },
  server: {
    proxy: {
      '/api': { target: process.env.VITE_API_PROXY_TARGET ?? 'http://localhost:3000', changeOrigin: true },
      '/admin': { target: 'http://localhost:5174', changeOrigin: true, ws: true },
      // The Svelte dev server is mounted at /admin/. Only the upstream
      // request is rewritten; the browser remains on /login for shared auth.
      '/login': { target: 'http://localhost:5174', changeOrigin: true, ws: true, rewrite: () => '/admin/' },
    },
  },
})
