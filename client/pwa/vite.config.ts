import { fileURLToPath, URL } from 'node:url'
import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

export default defineConfig({
  base: '/',
  plugins: [
    vue(),
    {
      name: 'redirect-client-login-to-admin-dev-server',
      configureServer(server) {
        server.middlewares.use((req, res, next) => {
          if (!req.url?.startsWith('/login')) return next()
          // 管理画面の Svelte モジュールは管理側 Vite サーバーから配信する。
          // HTML だけを別オリジン経由で返すと、開発時に画面が初期化されない。
          res.writeHead(302, { location: `http://localhost:5174${req.url}` })
          res.end()
        })
      },
    },
  ],
  resolve: { alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) } },
  server: {
    proxy: {
      '/api': { target: process.env.VITE_API_PROXY_TARGET ?? 'http://localhost:3000', changeOrigin: true },
      '/admin': { target: 'http://localhost:5174', changeOrigin: true, ws: true },
    },
  },
})
