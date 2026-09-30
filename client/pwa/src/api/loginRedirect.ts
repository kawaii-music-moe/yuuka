/**
 * 共有ログイン画面（管理画面 SPA）への導線。
 *
 * 管理画面は `/admin/` 配下でのみ配信される（`frontend/vite.config.ts` の `base: '/admin/'`・
 * Rust 側の `ADMIN_PREFIX`）。旧構成で共有ログインが居た `/login` はサーバー側に本体ルートが
 * 無く（互換リダイレクトのみ）、PWA からは最初から `/admin/login` を指す。
 *
 * `returnTo` は「同一オリジンの物理パス（+クエリ+ハッシュ）」。ログイン後、管理画面側は
 * `/admin/...` ならアプリ内遷移、それ以外（= この PWA のパス）ならフルページ遷移で戻す。
 * 管理画面側でも検証される（オープンリダイレクト対策）が、ここでも同一オリジンの絶対パス
 * 以外は渡さない。
 */
export const ADMIN_LOGIN_PATH = '/admin/login'

/** 同一オリジンの絶対パスか（`//host`・`/\host`・スキーム付き・相対パスは不可）。 */
function isSameOriginPath(path: string): boolean {
  return path.startsWith('/') && !path.startsWith('//') && !path.includes('\\')
}

/** 共有ログイン画面の URL。`returnTo` が安全な同一オリジンパスのときだけクエリへ載せる。 */
export function buildLoginUrl(returnTo?: string | null): string {
  if (!returnTo || !isSameOriginPath(returnTo)) return ADMIN_LOGIN_PATH
  return `${ADMIN_LOGIN_PATH}?returnTo=${encodeURIComponent(returnTo)}`
}

/** 現在の PWA 上の位置（パス+クエリ+ハッシュ）。ログイン後にここへ戻すために使う。 */
export function currentReturnTo(location: Pick<Location, 'pathname' | 'search' | 'hash'>): string {
  return `${location.pathname}${location.search}${location.hash}`
}
