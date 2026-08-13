import { readFile, stat } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath, URL } from 'node:url'
import { defineConfig, type Plugin } from 'vite'
import vue from '@vitejs/plugin-vue'

const adminRoot = fileURLToPath(new URL('../../src/public/', import.meta.url))
const adminMimeTypes: Record<string, string> = {
  '.css': 'text/css; charset=utf-8',
  '.html': 'text/html; charset=utf-8',
  '.ico': 'image/x-icon',
  '.js': 'application/javascript; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
  '.webp': 'image/webp',
}

function adminDevelopmentPlugin(): Plugin {
  return {
    name: 'yuuka-admin-development-server',
    configureServer(server) {
      server.middlewares.use((request, response, next) => {
        const pathname = new URL(request.url ?? '/', 'http://localhost').pathname
        if (pathname !== '/admin' && !pathname.startsWith('/admin/')) return next()

        void (async () => {
          const relativePath = pathname.slice('/admin'.length) || '/index.html'
          const requestedPath = path.normalize(path.join(adminRoot, relativePath))
          if (!requestedPath.startsWith(adminRoot)) return next()

          const extension = path.extname(requestedPath)
          let target = requestedPath
          try {
            if (!(await stat(target)).isFile()) throw new Error('not a file')
          } catch {
            if (extension) return next()
            target = path.join(adminRoot, 'index.html')
          }

          let content = await readFile(target)
          if (path.basename(target) === 'index.html') {
            content = Buffer.from(
              content
                .toString('utf8')
                .replaceAll('href="/', 'href="/admin/')
                .replaceAll('src="/', 'src="/admin/'),
            )
          }
          response.statusCode = 200
          response.setHeader('Content-Type', adminMimeTypes[path.extname(target)] ?? 'application/octet-stream')
          response.end(content)
        })().catch(next)
      })
    },
  }
}

export default defineConfig({
  base: '/',
  plugins: [adminDevelopmentPlugin(), vue()],
  resolve: { alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) } },
  server: {
    proxy: {
      '/api': { target: process.env.VITE_API_PROXY_TARGET ?? 'http://localhost:3000', changeOrigin: true },
    },
  },
})
