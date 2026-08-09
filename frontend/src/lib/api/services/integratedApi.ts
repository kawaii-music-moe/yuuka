// integratedApi — src/server/routes/integratedRoutes.ts に対応。
//
// overview は非スコープ（全 Bot 横断）だが、bot 操作系（start/stop/restart/clear-history）
// と grants 系は body.botId を読む。ただし呼び出し側が対象 botId を明示的に渡す設計のため、
// activeBot への自動フォールバックは望ましくない → scope:'user' で botId を body に明示。
//
// ★フィールド名はハンドラの ctx.body.* 読み取りに厳密一致させる（granted / serviceName / mode）。
import { api } from "../client";
import type {
	IntegratedOverviewResponse,
	GoogleCalendarsResponse,
	ApiResponse,
} from "../types";

const USER = { scope: "user" } as const;

/** 共有Botが利用するMCPサーバー 1 件（bot-canonical・付与者を問わず全許可分 + システム）。 */
export type BotMcpGrantServer = {
	id: number;
	name: string;
	/** サーバー有効フラグ。無効なら実行時に使われない。 */
	enabled: boolean;
	/** システムレベル登録（全Bot常時利用・解除不可）か。 */
	system: boolean;
	/** 自分が付与したものか（共有相手が付与したものは false）。 */
	mine: boolean;
};

/** 呼び出し元が当該Botへ付与できる「自分の」未付与サーバー 1 件。 */
export type BotMcpOwnServer = {
	id: number;
	name: string;
	endpoint_url: string;
	enabled: boolean;
	has_auth: boolean;
};

/** GET /api/integrated/bots/mcp のレスポンス。 */
export type BotMcpGrantsResponse = {
	success: boolean;
	bot_id: string;
	/** 呼び出し元がBotオーナー本人か（共有相手は false）。 */
	is_owner: boolean;
	servers: BotMcpGrantServer[];
	own_servers: BotMcpOwnServer[];
};

export const integratedApi = {
	/** GET /api/integrated/overview — 全 Bot 横断の統合ビュー */
	overview: () => api.get<IntegratedOverviewResponse>("/api/integrated/overview", USER),

	/**
	 * GET /api/integrated/bots/mcp?botId= — 共有Botが使うMCP（bot-canonical）と
	 * 付与できる自分の未付与サーバー。scope:'bot' で現在の activeBot を注入する。
	 */
	botMcpGrants: () =>
		api.get<BotMcpGrantsResponse>("/api/integrated/bots/mcp", { scope: "bot" }),

	// ── Bot ライフサイクル操作（対象 botId を明示） ──
	/** POST /api/integrated/bots/start */
	startBot: (botId: string) =>
		api.post<ApiResponse>("/api/integrated/bots/start", { botId }, USER),
	/** POST /api/integrated/bots/stop */
	stopBot: (botId: string) =>
		api.post<ApiResponse>("/api/integrated/bots/stop", { botId }, USER),
	/** POST /api/integrated/bots/restart */
	restartBot: (botId: string) =>
		api.post<ApiResponse>("/api/integrated/bots/restart", { botId }, USER),
	/** POST /api/integrated/bots/clear-history */
	clearHistory: (botId: string) =>
		api.post<ApiResponse>("/api/integrated/bots/clear-history", { botId }, USER),

	// ── 許可付与（grants）。ハンドラは ctx.body.granted / serviceName を読む ──
	/** POST /api/integrated/grants/mcp */
	grantMcp: (body: { botId: string; serverId: number; granted: boolean }) =>
		api.post<ApiResponse>("/api/integrated/grants/mcp", body, USER),
	/** POST /api/integrated/grants/credential */
	grantCredential: (body: {
		botId: string;
		serviceName: string;
		granted: boolean;
	}) => api.post<ApiResponse>("/api/integrated/grants/credential", body, USER),
	/** POST /api/integrated/grants/google — mode: "primary" | "none" | "account"(+accountId) */
	grantGoogle: (body: {
		botId: string;
		mode: "primary" | "none" | "account";
		accountId?: number;
	}) => api.post<ApiResponse>("/api/integrated/grants/google", body, USER),

	// ── Google アカウント管理 ──
	/** POST /api/integrated/google/accounts/primary */
	setPrimaryGoogleAccount: (accountId: number) =>
		api.post<ApiResponse>("/api/integrated/google/accounts/primary", { accountId }, USER),
	/** POST /api/integrated/google/accounts/delete */
	deleteGoogleAccount: (accountId: number) =>
		api.post<ApiResponse>("/api/integrated/google/accounts/delete", { accountId }, USER),
	/** POST /api/integrated/google/accounts/calendars — 同期カレンダー設定 */
	setGoogleCalendars: (body: { accountId: number; calendars: string[] }) =>
		api.post<ApiResponse>("/api/integrated/google/accounts/calendars", body, USER),
	/** GET /api/integrated/google/accounts/:id/calendars — カレンダー一覧取得 */
	googleAccountCalendars: (accountId: number) =>
		api.get<GoogleCalendarsResponse>(
			`/api/integrated/google/accounts/${accountId}/calendars`,
			USER,
		),
};
