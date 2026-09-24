// ─────────────────────────────────────────────────────────────────────────────
// 軽量 History ルーター（§2.2 / §8）
//
// 現 app.js の navigateTo()→applyRoute() の1対1構造を Svelte ストア + popstate 1個で
// 再現する。11ルート分類 + 15 Bot タブ。外部依存なし。
//
// - currentRoute: writable<string>（cleanPath 正規化済みの現パス。BASE_PATH は含まない）
// - page:         writable<URL>（/device の ?code= 用に searchParams へアクセス可能。実URL＝BASE_PATH を含む）
// - isPublicPath(path): §8 PUBLIC_PATHS（/usage,/terms,/privacy,/tasks/guide）を true
// - isAllowedReturnPath(path): returnTo として安全に使えるか判定（オープンリダイレクト対策・#34）
// - goto(path):  api client の 401 ハンドラ・各コンポーネントの遷移で使う（= navigateTo）
// - resolveRoute(url): §8 全パス → {view, tab?, params?} を解決
//
// 認可・プリセット別タブフィルタ・Bot 未選択リダイレクトはここに密結合させず、
// App.svelte / BotShell.svelte 側の認証ストア購読ガードで行う（§8 末尾）。
//
// #34（管理画面の /admin 移設）: 管理画面 SPA は `vite.config.ts` の `base:"/admin/"` 配下でのみ
// 配信される（Rust 側は `crates/yuuka-web/src/static_files.rs` が `/admin` に nest）。
//
// 「実URL」（location.pathname / pushState 先 / page ストア）は常に BASE_PATH を含み、
// 「アプリ相対パス」（currentRoute ストア・navigateTo/goto の引数・resolveRoute が返す view の元）は
// 常に BASE_PATH を含まない。この 2 つを取り違えると、アプリ内に実在するルート "/admin"
// （AdminOverlay。BotSelection の管理者リンク）と配信プレフィックス BASE_PATH="/admin" が
// 衝突し得るため、境界を厳密に本ファイル内へ閉じ込める:
//   - stripBasePath(): 実URL の pathname → アプリ相対パス（物理 URL 由来の入力にのみ適用する。
//     window.location 由来（初期化・popstate・resolveRoute への page 入力）でのみ呼ぶ）
//   - withBasePath():   アプリ相対パス → 実URL（pushState/href 用）
//   - cleanPath():      アプリ相対パスの正規化のみ（?/#/末尾スラッシュ除去）。BASE_PATH は
//     一切見ない＝純粋関数。navigateTo() 経由の入力（既にアプリ相対）にはこちらのみを適用する。
// こうすることで、BotShell 等の呼び出し側は一切 BASE_PATH を意識せず navigateTo("/bot/dashboard")
// のように書き続けられ、かつ navigateTo("/admin")（AdminOverlay への遷移）も正しく機能する。
// ─────────────────────────────────────────────────────────────────────────────

import { writable } from "svelte/store";

/**
 * 管理画面 SPA の配信プレフィックス。`vite.config.ts` の `base` から導出するため、両者が
 * ズレることはない（`import.meta.env.BASE_URL` は Vite が build/dev いずれも base と一致させて
 * 注入する）。末尾スラッシュは持たない（"/admin"）。base が "/"（プレフィックス無し）のときは
 * 空文字列になり、以降の付与/剥離処理は実質 no-op になる。
 */
const BASE_PATH = import.meta.env.BASE_URL.replace(/\/$/, "");

/**
 * 実 URL の pathname から BASE_PATH を剥がし、アプリ相対パスへ正規化する（冪等）。
 * **物理 URL（window.location 由来）にのみ**使う。navigateTo() 等が受け取るアプリ相対パス
 * （呼び出し側は BASE_PATH を付けない規約）にこれを適用すると、アプリ内に実在するルート
 * "/admin"（AdminOverlay）が BASE_PATH="/admin" と誤って一致し、"/" に潰れてしまう。
 */
function stripBasePath(pathname: string): string {
	if (!BASE_PATH) return pathname;
	if (pathname === BASE_PATH) return "/";
	if (pathname.startsWith(`${BASE_PATH}/`)) return pathname.slice(BASE_PATH.length) || "/";
	return pathname;
}

/**
 * アプリ相対パス（"/bot/dashboard" 等。クエリ付きも可）に BASE_PATH を付与し、ブラウザで実際に
 * 使える URL にする。`<a href>` や pushState に使う。
 *
 * 既に `${BASE_PATH}/...`（物理パス）の形であればそのまま返す（外部由来の returnTo 等、既に
 * 物理パスが渡ってくる場合の二重付与を防ぐ）。ただし **`path === BASE_PATH` 単独一致では
 * 早期リターンしない**: アプリには実在するルート "/admin"（AdminOverlay）があり、これは
 * BASE_PATH="/admin" と文字列として偶然一致する。ここで素通りさせると
 * withBasePath("/admin") が "/admin/admin" ではなく "/admin" を返してしまい、AdminOverlay への
 * 遷移が壊れる（物理 URL がアプリ相対パスと同じ＝実質ルート "/" に化ける）。
 */
export function withBasePath(path: string): string {
	if (!BASE_PATH) return path;
	if (path.startsWith(`${BASE_PATH}/`)) return path;
	return path === "/" ? `${BASE_PATH}/` : `${BASE_PATH}${path}`;
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
 *
 * **純粋関数**（BASE_PATH は見ない）。入力は常に「アプリ相対パス」（呼び出し側が既に
 * BASE_PATH を含めていない前提）。物理 URL の pathname を渡すときは、先に stripBasePath()
 * を通すこと（initRouter / resolveRoute 参照）。
 */
export function cleanPath(path: string): string {
	return path.split("?")[0].split("#")[0].replace(/\/$/, "") || "/";
}

/** §8 公開ルート判定（api client の 401 ハンドラが使用）。引数はアプリ相対パス。 */
export function isPublicPath(path: string): boolean {
	return (PUBLIC_PATHS as readonly string[]).includes(cleanPath(path));
}

/**
 * returnTo として安全に使えるアプリ相対パスかを判定する（オープンリダイレクト対策・#34 症状4）。
 *
 * 許可条件は「同一オリジンの絶対パス」のみ:
 *   - `/` で始まる（相対パス・スキーム付き URL は拒否）
 *   - `//`（スキーム相対 URL＝ホスト差し替え）は拒否
 *   - バックスラッシュを含まない（一部ブラウザの URL 解析でスキーム相対化に化けるため）
 *   - 上記を通過しても、実際に `URL` でオリジン固定パースした結果が同一オリジンであること
 *     （WHATWG URL の空白/タブ除去等で `//evil.com` 相当に化けるケースへの保険）
 *
 * Node 版（e2d0fe1 `isAllowedReturnPath`）は `/admin` プレフィックスの有無をロールで
 * ゲートしていたが、当時は `/admin` と Client(PWA) が別オリジン相当の別ディレクトリだったため。
 * 現行構成では「管理画面 SPA 内のどのビューを見せるか」は App.svelte の認可ゲート
 * （`effectiveView` の admin 判定等）が別途担うため、本関数はオープンリダイレクト防止のみに
 * 専念する（ロールは見ない）。
 */
export function isAllowedReturnPath(path: string | null | undefined): boolean {
	if (!path) return false;
	if (!path.startsWith("/") || path.startsWith("//")) return false;
	if (path.includes("\\")) return false;
	try {
		const url = new URL(path, "http://localhost");
		if (url.origin !== "http://localhost") return false;
	} catch {
		return false;
	}
	return true;
}

// ── ストア ────────────────────────────────────────────────────────────────
/** 現パス（cleanPath 正規化済み・アプリ相対）。物理 URL からは stripBasePath を経由する。 */
export const currentRoute = writable<string>(
	typeof window !== "undefined" ? cleanPath(stripBasePath(window.location.pathname)) : "/",
);

/** 現 URL（物理・searchParams アクセス用。/device の ?code= 等）。BASE_PATH を含む。 */
export const page = writable<URL>(
	typeof window !== "undefined"
		? new URL(window.location.href)
		: new URL("http://localhost/"),
);

/**
 * §8 全パス → ルート識別子を解決する。
 * 物理 URL（BASE_PATH を含む）または pathname 文字列を受ける（searchParams を保持したいので
 * URL 推奨。呼び出し元は `page` ストア＝常に物理 URL）。
 */
export function resolveRoute(input: URL | string): ResolvedRoute {
	const url =
		typeof input === "string"
			? new URL(input, typeof window !== "undefined" ? window.location.origin : "http://localhost")
			: input;
	const cp = cleanPath(stripBasePath(url.pathname));

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
 *
 * `path` は**アプリ相対パス**であること（物理 URL 由来の呼び出し元は先に stripBasePath を
 * 通すこと。initRouter 参照）。
 */
export function applyRoute(path: string): void {
	if (typeof window === "undefined") return;
	page.set(new URL(window.location.href));
	currentRoute.set(cleanPath(path));
}

/**
 * navigateTo（app.js:361-366 相当）。pushState してルート適用。
 * `path` はアプリ相対パス（例: "/bot/dashboard"）。物理 URL への変換（BASE_PATH 付与）は
 * ここで withBasePath() が行うので、呼び出し側は BASE_PATH を意識しない。
 */
export function navigateTo(path: string, pushState = true): void {
	if (typeof window === "undefined") return;
	if (pushState) {
		window.history.pushState({}, "", withBasePath(path));
	}
	applyRoute(path);
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
	// window.location は物理 URL（BASE_PATH を含む）→ stripBasePath でアプリ相対化してから
	// applyRoute に渡す（applyRoute はアプリ相対パスを前提とする）。
	const applyFromLocation = () => {
		applyRoute(stripBasePath(window.location.pathname) + window.location.search);
	};
	window.addEventListener("popstate", applyFromLocation);
	// 初期ルートを反映
	applyFromLocation();
	return () => window.removeEventListener("popstate", applyFromLocation);
}
