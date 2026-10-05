import { fileURLToPath, URL } from 'node:url'
import { defineConfig, type Plugin } from 'vite'
import vue from '@vitejs/plugin-vue'

// 管理画面（共有ログイン）は `frontend`（Svelte + Vite）の dev server が配信する。
// 旧 `src/public` 直下の静的ファイルを読む方式は撤去済みの admin UI を参照しており
// ENOENT → 500 になっていた（#44）。frontend の dev server はこの Client と同じ既定
// ポート(5173)を使うため衝突を避けて別ポートで起動する
// （scripts/dev-client-mock.mjs 参照）。
const adminDevServer = process.env.VITE_ADMIN_DEV_SERVER ?? 'http://localhost:5174'

function adminLoginRedirectPlugin(): Plugin {
  return {
    name: 'yuuka-admin-login-redirect',
    configureServer(server) {
      server.middlewares.use((request, response, next) => {
        const pathname = new URL(request.url ?? '/', 'http://localhost').pathname
        if (pathname !== '/login' && !pathname.startsWith('/login/')) return next()

        // 旧共有ログインのパス `/login` は本番の `GET /login` → `/admin/login` 互換
        // リダイレクトと同様に `/admin/login` へ写す（管理画面 SPA は `base: '/admin/'`
        // 配下でしか配信されない）。クエリ（`returnTo` 等）は保持する。
        // オリジンを付けない相対リダイレクトにして、トンネル等の公開ホスト名で開いたときに
        // localhost へ飛ばされないようにする。
        response.statusCode = 302
        response.setHeader('Location', `/admin${request.url}`)
        response.end()
      })
    },
  }
}

export default defineConfig({
  base: '/',
  plugins: [adminLoginRedirectPlugin(), vue()],
  resolve: { alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) } },
  server: {
    // dev を Cloudflare Tunnel 経由の公開ホスト名（https）で開くため Host ヘッダ検証を外す。
    // ホスト名をハードコードしないよう true にしている（dev server 専用・本番ビルドには影響しない）。
    allowedHosts: true,
    proxy: {
      // `/admin` 配下は frontend の dev server へ透過プロキシして同一オリジンに見せる
      // （本番も同一オリジンで Rust が /admin を配信する）。別オリジンへ redirect すると
      // トンネル経由のアクセスでも localhost:5174 へ飛ばされてしまう。frontend は
      // `base: '/admin/'` なので `/@vite/client`・`/src/...`・HMR の WebSocket も
      // すべて `/admin/` 配下になり、この Client のモジュールグラフとは衝突しない。
      '^/admin(/|$)': { target: adminDevServer, changeOrigin: true, ws: true },
      '/api': { target: process.env.VITE_API_PROXY_TARGET ?? 'http://localhost:3000', changeOrigin: true },
    },
  },
})
