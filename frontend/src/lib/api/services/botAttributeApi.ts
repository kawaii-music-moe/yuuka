// botAttributeApi — src/server/routes/botAttributeRoutes.ts に対応。
//
// ★注意: これらは全て /api/bots/* プレフィックスなので client の自動 botId 注入から
//   除外される（scope:'user'）。ハンドラは ctx.body.botId ?? query botId を自前で読むため、
//   呼び出し側が botId を明示的に渡す（GET は query、POST は body）。自動注入で上書きしない。
//   config タブ（芋づる: attributes/modules/assistant-config）が主な利用元。
import { api } from "../client";
import type { PresetsResponse, BotUsageResponse, ApiResponse } from "../types";

const USER = { scope: "user" } as const;

export const botAttributeApi = {
	/** GET /api/bots/presets */
	presets: () => api.get<PresetsResponse>("/api/bots/presets", USER),

	/** GET /api/bots/usage?botId= — 使用量（手動 botId） */
	usage: (botId: string) =>
		api.get<BotUsageResponse>("/api/bots/usage", { ...USER, query: { botId } }),

	/** POST /api/bots/attributes — Bot 属性（能力トグル等）更新 */
	updateAttributes: (body: { botId: string; [k: string]: unknown }) =>
		api.post<ApiResponse>("/api/bots/attributes", body, USER),

	/** GET /api/bots/modules?botId= — モジュール一覧 */
	modules: (botId: string) =>
		api.get<ApiResponse>("/api/bots/modules", { ...USER, query: { botId } }),
	/** POST /api/bots/modules — モジュール有効化設定 */
	updateModules: (body: { botId: string; [k: string]: unknown }) =>
		api.post<ApiResponse>("/api/bots/modules", body, USER),

	/** GET /api/bots/assistant-config?botId= — アシスタント設定一括取得 */
	assistantConfig: (botId: string) =>
		api.get<ApiResponse>("/api/bots/assistant-config", { ...USER, query: { botId } }),

	// ── assistant/* サブ設定 ──
	/** POST /api/bots/assistant/gemini-key */
	setGeminiKey: (body: { botId: string; apiKey: string }) =>
		api.post<ApiResponse>("/api/bots/assistant/gemini-key", body, USER),
	/** POST /api/bots/assistant/persona */
	setPersona: (body: { botId: string; personaId: number | null }) =>
		api.post<ApiResponse>("/api/bots/assistant/persona", body, USER),
	/** POST /api/bots/assistant/guilds */
	setGuilds: (body: { botId: string; [k: string]: unknown }) =>
		api.post<ApiResponse>("/api/bots/assistant/guilds", body, USER),
	/** POST /api/bots/assistant/channels — 有効化チャンネル（メンション不要で応答） */
	setChannels: (body: { botId: string; [k: string]: unknown }) =>
		api.post<ApiResponse>("/api/bots/assistant/channels", body, USER),
	/** POST /api/bots/assistant/members */
	setMembers: (body: { botId: string; [k: string]: unknown }) =>
		api.post<ApiResponse>("/api/bots/assistant/members", body, USER),
	/** POST /api/bots/assistant/roles */
	setRoles: (body: { botId: string; [k: string]: unknown }) =>
		api.post<ApiResponse>("/api/bots/assistant/roles", body, USER),
	/** GET /api/bots/assistant/guild-options?botId=&guildId= */
	guildOptions: (botId: string, query?: Record<string, string>) =>
		api.get<ApiResponse>("/api/bots/assistant/guild-options", {
			...USER,
			query: { botId, ...query },
		}),
	/** GET /api/bots/assistant/guild-note?botId= */
	getGuildNote: (botId: string, query?: Record<string, string>) =>
		api.get<ApiResponse>("/api/bots/assistant/guild-note", {
			...USER,
			query: { botId, ...query },
		}),
	/** POST /api/bots/assistant/guild-note */
	setGuildNote: (body: { botId: string; [k: string]: unknown }) =>
		api.post<ApiResponse>("/api/bots/assistant/guild-note", body, USER),

	/** POST /api/bots/recommended-persona — 推奨ペルソナ設定（personaRoutes 側） */
	setRecommendedPersona: (body: { botId: string; personaId: number | null }) =>
		api.post<ApiResponse>("/api/bots/recommended-persona", body, USER),

	// ── Bot共有（config タブ。botApi.shares の型が現行サーバ形状と異なるため
	//   config タブ用に targetUserId ベースで再定義する。§5.2） ──
	/** GET /api/bots/shares?botId= — 共有一覧＋推奨ペルソナID */
	shares: (botId: string) =>
		api.get<ApiResponse>("/api/bots/shares", { ...USER, query: { botId } }),
	/** POST /api/bots/shares/invite */
	inviteShare: (body: { botId: string; targetUserId: string }) =>
		api.post<ApiResponse>("/api/bots/shares/invite", body, USER),
	/** POST /api/bots/shares/revoke */
	revokeShare: (body: { botId: string; targetUserId: string }) =>
		api.post<ApiResponse>("/api/bots/shares/revoke", body, USER),

	// ── 利用申請（承認待ち）。decide はサーバが decision を読む（botApi 版は approve を送るため使わない） ──
	/** GET /api/bots/member-requests?status=pending&botId= — 承認待ちの利用申請一覧 */
	memberRequests: (botId: string, status = "pending") =>
		api.get<ApiResponse>("/api/bots/member-requests", {
			...USER,
			query: { botId, status },
		}),
	/** POST /api/bots/member-requests/:id/decide — 承認/却下（decision: approved|rejected） */
	decideMemberRequest: (requestId: number, decision: "approved" | "rejected") =>
		api.post<ApiResponse>(
			`/api/bots/member-requests/${encodeURIComponent(requestId)}/decide`,
			{ decision },
			USER,
		),

	// ── Discord プロフィール同期（招待リンクカードの「今すぐ同期」） ──
	/** POST /api/bots/sync-discord */
	syncDiscord: (botId: string) =>
		api.post<ApiResponse>("/api/bots/sync-discord", { botId }, USER),

	/** POST /api/bots/profile — Bot登録名の変更（name のみ送信。avatar は COALESCE で保持） */
	updateBotProfile: (body: { botId: string; name: string }) =>
		api.post<ApiResponse>("/api/bots/profile", body, USER),

	/** GET /api/bots — 所有＋共有 Bot 一覧（属性カードの現在値参照用） */
	botList: () => api.get<ApiResponse>("/api/bots", USER),
};
