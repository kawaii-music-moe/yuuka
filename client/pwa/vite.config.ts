import { fileURLToPath, URL } from 'node:url'
import { defineConfig, type Plugin } from 'vite'
import vue from '@vitejs/plugin-vue'

// 管理画面（共有ログイン）は `frontend`（Svelte + Vite）の dev server が配信する。
// 旧 `src/public` 直下の静的ファイルを読む方式は撤去済みの admin UI を参照しており
// ENOENT → 500 になっていた（#44）。frontend の dev server はこの Client と同じ既定
// ポート(5173)を使うため衝突を避けて別ポートで起動する
// （scripts/dev-client-mock.mjs 参照）。
const adminDevServer = process.env.VITE_ADMIN_DEV_SERVER ?? 'http://localhost:5174'

function adminDevelopmentRedirectPlugin(): Plugin {
  return {
    name: 'yuuka-admin-development-redirect',
    configureServer(server) {
      server.middlewares.use((request, response, next) => {
        const pathname = new URL(request.url ?? '/', 'http://localhost').pathname
        const isLogin = pathname === '/login' || pathname.startsWith('/login/')
        const isAdmin = pathname === '/admin' || pathname.startsWith('/admin/')
        if (!isLogin && !isAdmin) return next()

        // フル別オリジンへ遷移させる（プロキシではなく redirect）。frontend の Vite
        // dev server が返す HTML は `/@vite/client` や `/src/...` をルート相対で参照する
        // ため、この Client 自身の dev server 越しに透過プロキシすると HMR/モジュール
        // 解決がこの Client 側のモジュールグラフと衝突して壊れる。ブラウザ自体を
        // frontend の dev server オリジンへ移すことで、そちら側で正しく解決させる。
        //
        // 管理画面 SPA は `base: '/admin/'` 配下でしか配信されない（本番の Rust も同じ）。
        // 旧共有ログインのパス `/login` は本番の `GET /login` → `/admin/login` 互換
        // リダイレクトと同様に `/admin/login` へ写す（そのまま渡すと frontend dev server の
        // base 外＝404 になる）。クエリ（`returnTo` 等）は保持する。
        const target = isLogin ? `/admin${request.url}` : request.url
        response.statusCode = 302
        response.setHeader('Location', `${adminDevServer}${target}`)
        response.end()
      })
    },
  }
}

export default defineConfig({
  base: '/',
  plugins: [adminDevelopmentRedirectPlugin(), vue()],
  resolve: { alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) } },
  server: {
    proxy: {
      '/api': { target: process.env.VITE_API_PROXY_TARGET ?? 'http://localhost:3000', changeOrigin: true },
    },
  },
})
