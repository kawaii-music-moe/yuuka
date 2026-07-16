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
		// Service Worker は **無効化**（selfDestroying）。
		// 経緯: autoUpdate + precache + StaleWhileRevalidate の構成では、アプリを開いたまま
		//   新ビルドがデプロイされるとブラウザ内の旧 SW が旧ハッシュ資産を掴んだまま
		//   cleanupOutdatedCaches で旧キャッシュを消し、新旧 CSS/JS が skew して画面が総崩れになる
		//   （遅延ビュー〔統合管理/管理者〕が先に、続いて他ページも表示不能＝進行性の白画面）。
		//   Svelte + Vite 移行後は資産ハッシュが頻繁に変わるため再デプロイ毎に再発していた。
		// 対処: selfDestroying:true で「自己解除 + 全キャッシュ削除」する sw.js を配信する。
		//   既存の壊れた SW を持つ全クライアントが次回ロードで自動回復し、以後は SW なしの
		//   プレーン SPA として動く（社内管理ポータルのためオフライン/インストール性の喪失は許容）。
		// 再有効化する場合は selfDestroying を外し、下記 workbox 設定へ戻す（ただし更新 skew の
		//   恒久対策〔資産の StaleWhileRevalidate 撤去・更新フロー是正〕を併せて行うこと）。
		VitePWA({
			selfDestroying: true,
			// 更新フローの単一責任者。skipWaiting/clientsClaim は明示しない
			//（autoUpdate + skipWaiting + clientsClaim の三点セットは新旧チャンク混在→白画面を再誘発するため禁止）。
			registerType: "autoUpdate",
			// 登録は main.ts の registerSW で明示（inline script を出さない = CSP準拠）
			injectRegister: null,
			// 既存の frontend/public/manifest.json を尊重（自前の webmanifest を生成・注入しない）
			manifest: false,
			workbox: {
				// Workbox ランタイムを sw.js にインライン化し
				// https://storage.googleapis.com/workbox-cdn の importScripts を消す（唯一の CSP 準拠要件）。
				inlineWorkboxRuntime: true,
				// precache manifest は Vite のハッシュ資産から自動生成（固定パス列挙を廃止）。
				// ★ html は precache しない（GSV 実行時置換をバイパスさせないため。注記(a)）。
				globPatterns: ["**/*.{js,css,woff2,png,svg,webp,json,ico}"],
				navigateFallback: "/index.html",
				// /api・/hook・/ws・/proxy を navigateFallback から除外
				navigateFallbackDenylist: [
					/^\/api\//,
					/^\/hook\//,
					/^\/ws\//,
					/^\/proxy\//,
				],
				runtimeCaching: [
					// index.html / "/" は precache せず NetworkFirst のみ（GSV 置換を必ず通す。注記(a)）
					{
						urlPattern: ({ url }) =>
							url.origin === self.location.origin &&
							!/^\/(api|hook|ws|proxy)\//.test(url.pathname) &&
							(url.pathname === "/" || url.pathname === "/index.html"),
						handler: "NetworkFirst",
						options: { cacheName: "app-shell" },
					},
					// その他 same-origin 静的資産（/api・/hook・/ws・/proxy は否定条件で除外）
					{
						urlPattern: ({ url }) =>
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
