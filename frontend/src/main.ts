import { mount } from "svelte";
import { registerSW } from "virtual:pwa-register";
import App from "./App.svelte";
import "./styles.css";

// Service Worker 登録（P1a: vite-plugin-pwa / Workbox generateSW）。
// virtual:pwa-register は Vite バンドルに含まれる 'self' 由来 module のため CSP 適合（inline script なし）。
// 更新戦略は vite.config.ts の registerType:"autoUpdate" に一任（skipWaiting/clientsClaim は明示しない）。
// 旧 src/public/sw.js（yuuka-v10）は参照せず、新 sw.js が cleanupOutdatedCaches で旧キャッシュを掃除する。
registerSW({ immediate: true });

// 遅延ロードチャンクの取得失敗からの自動復帰（デプロイ直後の stale ハッシュ対策）。
// autoUpdate + cleanupOutdatedCaches の構成では、アプリを開いたまま新ビルドがデプロイされると、
// メモリ上の旧 entry が参照する旧チャンクハッシュ（例: IntegratedOverlay-<oldhash>.js）が
// サーバ/キャッシュから消え、未ロードの遅延ビュー（統合管理・管理者など）へ初めて遷移した瞬間に
// 動的 import が 404 で失敗する＝画面が出ない。Vite は動的 import 失敗時に window へ
// `vite:preloadError` を投げるので、index.html を取り直して新しい entry + 正しいハッシュにするため
// 一度だけリロードする。無限ループ防止に sessionStorage で直近リロードをガードする（本当に消えた
// チャンクのときは LazyView の :catch が手動リロードを案内する）。
window.addEventListener("vite:preloadError", () => {
	const KEY = "yuuka:chunk-reload-at";
	const now = Date.now();
	const last = Number(sessionStorage.getItem(KEY) ?? "0");
	if (now - last < 10_000) return; // 直近 10 秒に復旧試行済みなら再リロードしない
	sessionStorage.setItem(KEY, String(now));
	location.reload();
});

const target = document.getElementById("app");
if (!target) throw new Error("#app mount target not found");

const app = mount(App, { target });

export default app;
