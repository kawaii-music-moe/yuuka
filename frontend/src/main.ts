import { mount } from "svelte";
import { registerSW } from "virtual:pwa-register";
import App from "./App.svelte";
import "./styles.css";

// Service Worker 登録（P1a: vite-plugin-pwa / Workbox generateSW）。
// virtual:pwa-register は Vite バンドルに含まれる 'self' 由来 module のため CSP 適合（inline script なし）。
// 更新戦略は vite.config.ts の registerType:"prompt"（skipWaiting/clientsClaim 無効）。
// onNeedRefresh を渡さないので UI プロンプトは出さず、新SWは全タブ離脱まで waiting に留まる
// = セッション途中で遅延チャンクをすげ替えない（白画面防止）。更新は次回フルロードで反映。
// 旧 src/public/sw.js（yuuka-v10）は参照せず、新 sw.js が cleanupOutdatedCaches で旧キャッシュを掃除する。
registerSW({ immediate: true });

// デプロイ跨ぎの自己回復: 遅延ロードした route チャンク（import()）が 404 等で失敗すると
// Vite が window に "vite:preloadError" を発火する。旧SW/旧index.html を掴んだままの
// クライアント（本修正の配信前から壊れているタブ）が MCP・Discord連携などのタブを開くと
// ChunkLoadError で「何も出ない」状態になるため、1回だけフルリロードして
// 新 index.html → 新 hash のチャンクで確実に解決させる。
// 10秒以内の連続発火は抑止し、真のオフライン等での無限リロードを防ぐ。
window.addEventListener("vite:preloadError", () => {
	const KEY = "yuuka:chunk-reload-at";
	const last = Number(sessionStorage.getItem(KEY) ?? 0);
	if (Date.now() - last < 10_000) return;
	sessionStorage.setItem(KEY, String(Date.now()));
	window.location.reload();
});

const target = document.getElementById("app");
if (!target) throw new Error("#app mount target not found");

const app = mount(App, { target });

export default app;
