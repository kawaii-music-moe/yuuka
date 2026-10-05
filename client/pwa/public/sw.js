// キャッシュ名にバージョンを含める。デプロイでハッシュ付き資産の中身が変わったときは
// このバージョンを上げる（activate で旧キャッシュを削除する）。#40: 固定キャッシュ名
// のままだと古いキャッシュが永久に残り、白画面の原因になっていた。
const VERSION = 'v2';
const CACHE = `agent-desk-${VERSION}`;
// '/' はここに含めない。ナビゲーションは常にネットワーク優先（下の fetch ハンドラ）
// なので、事前キャッシュすると逆に古い index.html を掴む危険がある。
const ASSETS = ['/manifest.webmanifest', '/icons/app-icon.svg'];

self.addEventListener('install', (event) => {
  event.waitUntil(caches.open(CACHE).then((cache) => cache.addAll(ASSETS)));
  self.skipWaiting();
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((key) => key !== CACHE).map((key) => caches.delete(key))))
      .then(() => self.clients.claim()),
  );
});

// 管理画面（共有ログイン含む）はこの PWA とは別アプリ。同一オリジン・同一スコープ('/')
// に間借りしているため SW 自体は避けられないが、fetch を横取りしないことで実質的に
// このキャッシュ戦略の影響範囲から外す（#40 付随課題: 管理画面を開いても PWA の SW が
// キャッシュを差し込んでしまう問題への対処）。
function isAdminPath(pathname) {
  return pathname === '/login' || pathname.startsWith('/login/') || pathname === '/admin' || pathname.startsWith('/admin/');
}

self.addEventListener('fetch', (event) => {
  const { request } = event;
  if (request.method !== 'GET') return;
  const { pathname } = new URL(request.url);
  if (pathname.startsWith('/api/') || isAdminPath(pathname)) return;

  if (request.mode === 'navigate') {
    // ナビゲーションはネットワーク優先。デプロイで資産のハッシュ名が変わっても、
    // 古い HTML が削除済みの JS/CSS を参照して白画面になることを防ぐ。オフライン時
    // のみキャッシュへフォールバックする。
    event.respondWith(
      fetch(request)
        .then((response) => {
          const copy = response.clone();
          caches.open(CACHE).then((cache) => cache.put(request, copy));
          return response;
        })
        // オフライン等でネットワークが失敗した場合のみキャッシュへフォールバックする。
        // 何もキャッシュされていない初回オフライン訪問でも respondWith に undefined を
        // 渡さないよう、最後は Response.error() で必ず Response を返す。
        .catch(async () => (await caches.match(request)) ?? (await caches.match('/')) ?? Response.error()),
    );
    return;
  }

  // 静的資産: Vite がハッシュ付きファイル名を発行するため cache-first で問題ない。
  // manifest/icons 等ハッシュなし資産は stale-while-revalidate で追従させる。
  event.respondWith(
    caches.match(request).then((cached) => {
      const network = fetch(request)
        .then((response) => {
          if (response.ok) caches.open(CACHE).then((cache) => cache.put(request, response.clone()));
          return response;
        })
        .catch(() => cached ?? Response.error());
      return cached ?? network;
    }),
  );
});
