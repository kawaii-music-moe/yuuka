// ─────────────────────────────────────────────────────────────────────────────
// 型付き API クライアント（§10.2）
//
// 現行 app.js の native fetch グローバルモンキーパッチ（app.js:25-65）を廃止し、
// 明示 apiClient に置換する。移行の最重要ポイント。
//
// 核心の設計:
//   - botId 注入は「opt-out（除外リスト以外は全注入）」を型で再現する。
//     opt-in（フラグを渡した時だけ注入）は注入漏れで bot-scoped API が silent に
//     system_default へ落ちる危険があるため採用しない。→ `scope` を Opts で必須化。
//   - scope:'bot'  … botId を必ず注入（GET/FormData は query、JSON body は body）。
//                     サーバは body 優先で両対応（todo/finance/schedule/delivery/
//                     botAttribute Routes で確認済み）。
//   - scope:'user' … botId を絶対に注入しない（auth/me/bots系/device/setup/admin/
//                     settings）。stale な botId を混ぜると別 Bot のデータに化ける。
//   - 除外は「プレフィックス一致」なので、サービス層で正しい scope を付ける責務。
//     （/api/bots 配下・/api/login|register|logout|me・/api/auth/device/*・
//       device-management 3本・/api/setup* は scope:'user'）
//
//   - credentials:'same-origin' を明示。dev は Vite proxy(changeOrigin:false)で
//     same-origin を維持し Cookie が通る。
//   - 401 集中割り込み: currentUser=null にした上で、bootstrap プローブ・公開ルート
//     上では /login へ遷移しない（匿名として扱う）。
//   - HTTP 200 でも success:false があり得る → 成否は res.ok と data.success の複合判定。
//
// import 先（activeBot/session/router）は他担当が新設する。存在前提で import する
// （統合時に解決される）。
// ─────────────────────────────────────────────────────────────────────────────

import { get } from "svelte/store";
import { currentRoute, goto, isPublicPath } from "$lib/router";
import { activeBot } from "$lib/stores/activeBot";
import { currentUser } from "$lib/stores/session";

/** scope:'bot' は botId 注入必須、'user' は絶対に注入しない。 */
export type Scope = "bot" | "user";

export interface RequestOpts {
	/** ★必須。省略不可（型で bot スコープ要否を強制）。 */
	scope: Scope;
	/** object（JSON化） | FormData（multipart） */
	body?: unknown;
	/** クエリパラメータ（botId 以外の任意パラメータ） */
	query?: Record<string, string | number | undefined>;
	/** 起動時 /api/me プローブ等: 401 でも /login へ遷移しない */
	isBootstrap?: boolean;
}

/** body を取らないメソッド（GET/DELETE）用の Opts。 */
export type NoBodyOpts = Omit<RequestOpts, "body">;

export class ApiError extends Error {
	constructor(
		public status: number,
		message: string,
	) {
		super(message);
		this.name = "ApiError";
	}
}

function applyQuery(url: URL, query?: RequestOpts["query"]): void {
	if (!query) return;
	for (const [k, v] of Object.entries(query)) {
		if (v !== undefined) url.searchParams.set(k, String(v));
	}
}

async function request<T>(
	method: string,
	path: string,
	opts: RequestOpts,
): Promise<T & { success?: boolean; message?: string }> {
	const url = new URL(path, location.origin);
	const botId = opts.scope === "bot" ? get(activeBot)?.id : undefined;
	const isForm = opts.body instanceof FormData;

	applyQuery(url, opts.query);

	// botId 注入:
	//   GET / FormData(multipart) → query（body を JSON 化しないため）
	//   それ以外（JSON body）      → body（サーバは body 優先で両対応）
	let finalBody: BodyInit | undefined;
	const headers: Record<string, string> = {};

	if (isForm) {
		if (botId) url.searchParams.set("botId", botId);
		// Content-Type はブラウザに委ねる（multipart 境界を自動付与）
		finalBody = opts.body as FormData;
	} else if (method === "GET") {
		if (botId) url.searchParams.set("botId", botId);
		finalBody = undefined;
	} else {
		const base =
			(opts.body as Record<string, unknown> | undefined) ??
			(botId ? {} : undefined);
		const merged = base && botId ? { ...base, botId } : base;
		if (merged !== undefined) {
			finalBody = JSON.stringify(merged);
			headers["Content-Type"] = "application/json";
		}
	}

	const res = await fetch(url.toString(), {
		method,
		credentials: "same-origin",
		headers,
		body: finalBody,
	});

	// 集中型 401: bootstrap プローブ・公開ルート上では遷移しない（匿名として扱う）
	if (res.status === 401) {
		currentUser.set(null);
		if (!opts.isBootstrap && !isPublicPath(get(currentRoute))) {
			goto("/login");
		}
		throw new ApiError(401, "認証が必要です");
	}

	const data = (await res.json().catch(() => ({}))) as {
		success?: boolean;
		message?: string;
	} & T;

	// HTTP エラー、または 200 でも success:false は失敗扱い
	if (!res.ok || data.success === false) {
		throw new ApiError(res.status, data.message ?? "エラーが発生しました");
	}
	return data;
}

export const api = {
	get: <T>(path: string, opts: NoBodyOpts) =>
		request<T>("GET", path, opts as RequestOpts),
	post: <T>(path: string, body?: unknown, opts?: NoBodyOpts) =>
		request<T>("POST", path, { ...(opts as RequestOpts), body }),
	del: <T>(path: string, opts: NoBodyOpts) =>
		request<T>("DELETE", path, opts as RequestOpts),
};
