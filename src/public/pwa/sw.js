const CACHE = 'agent-desk-v1';
const ASSETS = ['/', '/manifest.webmanifest', '/icons/app-icon.svg'];
self.addEventListener('install', (event) => event.waitUntil(caches.open(CACHE).then((cache) => cache.addAll(ASSETS))));
self.addEventListener('fetch', (event) => { if (event.request.method === 'GET' && !new URL(event.request.url).pathname.startsWith('/api/')) event.respondWith(caches.match(event.request).then((cached) => cached ?? fetch(event.request))); });
