// ─────────────────────────────────────────────────────────────────────────────
// 軽量 History ルーター（§2.2 / §8）
//
// 現 app.js の navigateTo()→applyRoute() の1対1構造を Svelte ストア + popstate 1個で
// 再現する。11ルート分類 + 15 Bot タブ。外部依存なし。
//
// - currentRoute: writable<string>（cleanPath 正規化済みの現パス）
// - page:         writable<URL>（/device の ?code= 用に searchParams へアクセス可能）
// - isPublicPath(path): §8 PUBLIC_PATHS（/usage,/terms,/privacy,/tasks/guide）を true
// - goto(path):  api client の 401 ハンドラ・各コンポーネントの遷移で使う（= navigateTo）
// - resolveRoute(url): §8 全パス → {view, tab?, params?} を解決
//
// 認可・プリセット別タブフィルタ・Bot 未選択リダイレクトはここに密結合させず、
// App.svelte / BotShell.svelte 側の認証ストア購読ガードで行う（§8 末尾）。
// ─────────────────────────────────────────────────────────────────────────────

import { writable } from "svelte/store";

export const ADMIN_PREFIX = "/admin";

export function adminPath(path: string): string {
	if (path === "/login" || path.startsWith("/login?")) return path;
	if (
		path === ADMIN_PREFIX ||
		path.startsWith(`${ADMIN_PREFIX}/`) ||
		path.startsWith(`${ADMIN_PREFIX}?`)
	) return path;
	return `${ADMIN_PREFIX}${path.startsWith("/") ? path : `/${path}`}`;
}

/** §8 対応表のルート識別子。 */
export type RouteView =
	| "bots" // /, /bots, /index.html
	| "login" // /login
	| "bot" // /bot, /bot/<tab>
	| "integrated" // /integrated
	| "admin" // /admin
	| "account" // /account
	| "device" // /device
	| "usage" // /usage
	| "terms" // /terms
	| "privacy" // /privacy
	| "tasks-guide" // /tasks/guide
	| "notfound"; // 未知パス

/** /bot/<tab> の 16 タブ識別子（§8 + 設定ハブ "settings"）。 */
export type BotTab =
	| "dashboard"
	| "tasks"
	| "timeline"
	| "schedules"
	| "expenses"
	| "reminders"
	| "personal"
	| "personas"
	| "delivery"
	| "webhooks"
	| "mcp"
	| "playbooks"
	| "discord"
	| "config"
	| "devices"
	| "settings";

export interface ResolvedRoute {
	view: RouteView;
	/** view === "bot" のときのみ設定される選択タブ。 */
	tab?: BotTab;
	/** クエリ等から抽出したパラメータ（例: /device?code=... の code）。 */
	params?: Record<string, string>;
}

/** §8 の 16 Bot タブ（+ 設定ハブ）。未知タブ → "config" フォールバック。 */
export const BOT_TABS: BotTab[] = [
	"dashboard",
	"tasks",
	"timeline",
	"schedules",
	"expenses",
	"reminders",
	"personal",
	"personas",
	"delivery",
	"webhooks",
	"mcp",
	"playbooks",
	"discord",
	"config",
	"devices",
	"settings",
];

/** §8: /bot 直下・未知タブ時の既定タブ（BotShell の loader フォールバックも共用）。 */
export const DEFAULT_BOT_TAB: BotTab = "config";

/** §8 PUBLIC_PATHS: 認証を待たず描画できる公開ルート。 */
export const PUBLIC_PATHS = ["/usage", "/terms", "/privacy", "/tasks/guide"] as const;

/**
 * cleanPath 正規化（app.js:369-370 流用）。
 * ?・#・末尾スラッシュを除去。空になれば "/"。
 */
export function cleanPath(path: string): string {
	return path.split("?")[0].split("#")[0].replace(/\/$/, "") || "/";
}

/** §8 公開ルート判定（api client の 401 ハンドラが使用）。 */
export function isPublicPath(path: string): boolean {
	return (PUBLIC_PATHS as readonly string[]).includes(cleanPath(path));
}

// ── ストア ────────────────────────────────────────────────────────────────
/** 現パス（cleanPath 正規化済み）。 */
export const currentRoute = writable<string>(
	typeof window !== "undefined" ? cleanPath(window.location.pathname) : "/",
);

/** 現 URL（searchParams アクセス用。/device の ?code= 等）。 */
export const page = writable<URL>(
	typeof window !== "undefined"
		? new URL(window.location.href)
		: new URL("http://localhost/"),
);

/**
 * §8 全パス → ルート識別子を解決する。
 * URL または パス文字列を受ける（searchParams を保持したいので URL 推奨）。
 */
export function resolveRoute(input: URL | string): ResolvedRoute {
	const url =
		typeof input === "string"
			? new URL(input, typeof window !== "undefined" ? window.location.origin : "http://localhost")
			: input;
	const requestedPath = cleanPath(url.pathname);
	const isLogin = requestedPath === "/login";
	if (!isLogin && requestedPath !== ADMIN_PREFIX && !requestedPath.startsWith(`${ADMIN_PREFIX}/`)) {
		return { view: "notfound" };
	}
	const cp = isLogin
		? "/login"
		: requestedPath.slice(ADMIN_PREFIX.length) || "/";

	// 公開ページ
	if (cp === "/usage") return { view: "usage" };
	if (cp === "/terms") return { view: "terms" };
	if (cp === "/privacy") return { view: "privacy" };
	if (cp === "/tasks/guide") return { view: "tasks-guide" };

	// 認証・独立オーバーレイ
	if (cp === "/login") return { view: "login" };
	if (cp === "/integrated") return { view: "integrated" };
	if (cp === "/admin") return { view: "admin" };
	if (cp === "/account") return { view: "account" };
	if (cp === "/device") {
		const code = url.searchParams.get("code");
		return { view: "device", params: code ? { code } : {} };
	}

	// Bot 選択（エイリアス3つ）
	if (cp === "/" || cp === "/bots" || cp === "/index.html") {
		return { view: "bots" };
	}

	// Bot 個別画面 /bot, /bot/<tab>
	if (cp === "/bot" || cp.startsWith("/bot/")) {
		// "/bot/".length === 5（app.js:561）。既定・未知タブ → DEFAULT_BOT_TAB。
		let tabId = cp === "/bot" ? DEFAULT_BOT_TAB : cp.slice(5);
		if (!BOT_TABS.includes(tabId as BotTab)) tabId = DEFAULT_BOT_TAB;
		return { view: "bot", tab: tabId as BotTab };
	}

	// 未知パス（ガードは App 側: ログイン済→/、未ログイン→/login）
	return { view: "notfound" };
}

/**
 * ルート適用（applyRoute 相当）。currentRoute / page ストアを更新するだけ。
 * 実際の表示切替は App.svelte の {#if}/<svelte:component> が購読で行う。
 */
export function applyRoute(path: string): void {
	if (typeof window === "undefined") return;
	page.set(new URL(window.location.href));
	currentRoute.set(cleanPath(path));
}

/**
 * navigateTo（app.js:361-366 相当）。pushState してルート適用。
 */
export function navigateTo(path: string, pushState = true): void {
	if (typeof window === "undefined") return;
	const destination = adminPath(path);
	if (pushState) {
		window.history.pushState({}, "", destination);
	}
	applyRoute(destination);
}

/** goto: api client の 401 ハンドラ・各コンポーネントが使う navigateTo エイリアス。 */
export function goto(path: string): void {
	navigateTo(path, true);
}

/**
 * popstate リスナ1個を登録する初期化関数。App.svelte の onMount で1回呼ぶ。
 * 戻り値はクリーンアップ関数（onDestroy 用）。
 */
export function initRouter(): () => void {
	if (typeof window === "undefined") return () => {};
	const onPop = () => {
		applyRoute(window.location.pathname + window.location.search);
	};
	window.addEventListener("popstate", onPop);
	// 初期ルートを反映
	applyRoute(window.location.pathname + window.location.search);
	return () => window.removeEventListener("popstate", onPop);
}
