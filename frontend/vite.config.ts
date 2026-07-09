import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vite";
import { VitePWA } from "vite-plugin-pwa";

// dev-hot(compose)標準フロー = 7855 / ホスト直 tsx watch(dev:host) = 7854
// prod の HOST_PORT は環境毎に異なる(dev=7855, prod=7701 等)ため、必ず env で切替可能に
const API = process.env.VITE_API_TARGET ?? "http://127.0.0.1:7855";

// P1a: vite-plugin-pwa の virtual:pwa-register は `workbox-window` を import するが、
// pnpm の厳格 hoist 配置では bare specifier が top-level から解決できず rolldown が
// "failed to resolve import workbox-window" で落ちる。plugin のネストした dep を絶対パスで
// 解決して alias する（fresh install / .pnpm symlink いずれのレイアウトでも動く版）。
const require = createRequire(import.meta.url);
function resolveWorkboxWindow(): string | undefined {
	// 1) 通常解決（hoist されていれば効く）
	try {
		return require.resolve("workbox-window");
	} catch {
		/* fall through */
	}
	// 2) .pnpm ストア内をバージョン非依存でグロブ
	try {
		const pnpmDir = path.resolve(__dirname, "../node_modules/.pnpm");
		const dir = fs
			.readdirSync(pnpmDir)
			.find((d) => /^workbox-window@/.test(d));
		if (dir) {
			const pkgJson = path.join(
				pnpmDir,
				dir,
				"node_modules/workbox-window/package.json",
			);
			return createRequire(pkgJson).resolve("workbox-window");
		}
	} catch {
		/* fall through */
	}
	return undefined;
}
const workboxWindowPath = resolveWorkboxWindow();

export default defineConfig({
	root: __dirname,
	base: "/", // ★デフォルト維持（/theme-init.js 等の絶対パス参照が書き換わらないよう）
	publicDir: "public",
	build: {
		outDir: "../dist/public",
		emptyOutDir: true, // dist/public 配下のみクリア（dist/ 直下の tsgo 出力は無事）
		assetsDir: "assets", // ★固定: ハッシュ資産を assets/ 直下に集約（chunk 含む）
		assetsInlineLimit: 0, // CSP script-src 'self' 準拠: inline module/data-URI を出さない
	},
	plugins: [
		svelte(),
		// P1a: Service Worker（Workbox generateSW / self-host / CSP準拠）
		// 旧 src/public/sw.js（yuuka-v10, 固定 /app.js precache）を置換。
		VitePWA({
			// 更新フローの単一責任者。
			// ★重要（不具合修正）: registerType:"autoUpdate" は vite-plugin-pwa が
			//   generateSW に skipWaiting:true / clientsClaim:true を*強制注入*する
			//   （node_modules/vite-plugin-pwa/dist/index.js:874-876。injectRegister==null
			//    かつ autoUpdate のとき workbox 側の指定を上書きして true 固定）。
			//   この三点セット（autoUpdate + skipWaiting + clientsClaim）は、ルート遅延
			//   ロード（§P1b の 29チャンク）と cleanupOutdatedCaches の併用時に
			//   「デプロイ直後、開いていた旧タブが旧hashの遅延チャンクを import → 新イメージ
			//    側で 404 → そのタブが白画面/何も出ない」を誘発する（例: MCP・Discord連携）。
			//   → registerType を "prompt" にして強制注入条件を外す。新SWは全タブが閉じる
			//     まで waiting に留まり、セッション途中でチャンクをすげ替えない
			//     （旧SWが旧チャンクを供給し続ける）。更新は次回フルロードで安全に適用。
			//   ※ main.ts は onNeedRefresh 無しで registerSW するため UI プロンプトは出ない
			//     （= 実質サイレント待機。旧 autoUpdate の即時反映は捨て、安全側に倒す）。
			registerType: "prompt",
			// 登録は main.ts の registerSW で明示（inline script を出さない = CSP準拠）
			injectRegister: null,
			// 既存の frontend/public/manifest.json を尊重（自前の webmanifest を生成・注入しない）
			manifest: false,
			workbox: {
				// 明示的に skipWaiting/clientsClaim を無効化（registerType:"prompt" では
				// プラグインの強制注入が働かないため、この指定が実際に効く）。生成 sw.js に
				// self.skipWaiting()/clients.claim() を入れないことで、遅延チャンク +
				// cleanupOutdatedCaches のセッション途中すげ替え（白画面）を根絶する。
				skipWaiting: false,
				clientsClaim: false,
				// Workbox ランタイムを sw.js にインライン化し
				// https://storage.googleapis.com/workbox-cdn の importScripts を消す（唯一の CSP 準拠要件）。
				inlineWorkboxRuntime: true,
				// precache manifest は Vite のハッシュ資産から自動生成（固定パス列挙を廃止）。
				// ★ html は precache しない（GSV 実行時置換をバイパスさせないため。注記(a)）。
				globPatterns: ["**/*.{js,css,woff2,png,svg,webp,json,ico}"],
				// ★ navigateFallback は使わない（不具合修正）。
				//   navigateFallback:"/index.html" は Workbox が SW 初期化時に
				//   precache.createHandlerBoundToURL("/index.html") を呼ぶが、html は GSV 実行時
				//   置換のため意図的に precache 対象外（globPatterns に html 無し）。結果
				//   "non-precached-url: /index.html" を throw し SW が壊れる（この方式と両立しない）。
				//   代わりに下の NetworkFirst ルールが request.mode==="navigate" の全 SPA 遷移
				//   （/, /index.html, /bot/... 等）を捌く。オンラインは常に最新 index.html（GSV 置換済）、
				//   オフラインは訪問済みルートのキャッシュへフォールバック。
				//   ★キー省略では不十分: vite-plugin-pwa は defaultWorkbox
				//     { navigateFallback: "index.html" } を Object.assign でマージするため
				//     （dist/index.js:838,858）、null を明示して初めてデフォルト注入が消える。
				navigateFallback: null,
				runtimeCaching: [
					// SPA ナビゲーション（HTML 遷移）は NetworkFirst（GSV 置換を必ず通す。注記(a)）
					{
						urlPattern: ({ request, url }) =>
							request.mode === "navigate" &&
							url.origin === self.location.origin &&
							!/^\/(api|hook|ws|proxy)\//.test(url.pathname),
						handler: "NetworkFirst",
						options: { cacheName: "app-shell" },
					},
					// その他 same-origin 静的資産（ナビゲーション以外。/api・/hook・/ws・/proxy は除外）
					{
						urlPattern: ({ request, url }) =>
							request.mode !== "navigate" &&
							url.origin === self.location.origin &&
							!/^\/(api|hook|ws|proxy)\//.test(url.pathname),
						handler: "StaleWhileRevalidate",
						options: { cacheName: "static" },
					},
				],
				// 旧キャッシュ掃除（旧 yuuka-v10 含む）
				cleanupOutdatedCaches: true,
			},
		}),
	],
	resolve: {
		alias: {
			// SvelteKit 風 import 記法（$lib/...）を frontend/src/lib へ解決
			$lib: path.resolve(__dirname, "src/lib"),
			// P1a: pnpm 厳格 hoist 環境で virtual:pwa-register の workbox-window import を解決
			...(workboxWindowPath ? { "workbox-window": workboxWindowPath } : {}),
		},
	},
	server: {
		port: 5173,
		// 真に必要なのは /api と /ws/chat のみ（§5.6 参照）
		proxy: {
			"/api": { target: API, changeOrigin: false },
			"/ws/chat": {
				target: API.replace("http", "ws"),
				ws: true,
				changeOrigin: false,
			},
		},
	},
});
