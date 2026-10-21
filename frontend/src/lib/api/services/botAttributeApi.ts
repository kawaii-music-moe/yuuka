// botAttributeApi — src/server/routes/botAttributeRoutes.ts に対応。
//
// ★注意: これらは全て /api/bots/* プレフィックスなので client の自動 botId 注入から
//   除外される（scope:'user'）。ハンドラは ctx.body.botId ?? query botId を自前で読むため、
//   呼び出し側が botId を明示的に渡す（GET は query、POST は body）。自動注入で上書きしない。
//   config タブ（芋づる: attributes/modules/assistant-config）が主な利用元。
//   botApi と重複する endpoint は定義を持たず、botApi のメソッドを互換名で再公開する（#161）。
import { api } from "../client";
import type { ApiResponse } from "../types";
import { botApi } from "./botApi";

const USER = { scope: "user" } as const;

export const botAttributeApi = {
	// 互換名: botApi と同じ endpoint は botApi の定義を直接参照する（#161）。
	presets: botApi.presets,
	usage: botApi.usage,

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
		api.get<ApiResponse>("/api/bots/assistant-config", {
			...USER,
			query: { botId },
		}),

	setLlm: (body: {
		botId: string;
		provider?: string;
		model?: string;
		inheritDefaults?: boolean;
	}) => api.post<ApiResponse>("/api/bots/assistant/llm", body, USER),

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
	/** POST /api/bots/assistant/muted-channels — 発言禁止チャンネル（メンション/返信があっても応答しない） */
	setMutedChannels: (body: { botId: string; [k: string]: unknown }) =>
		api.post<ApiResponse>("/api/bots/assistant/muted-channels", body, USER),
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

	// 共有・利用申請・Discord 同期・プロフィール・一覧も botApi の定義を参照する（互換名）。
	shares: botApi.shares,
	inviteShare: botApi.inviteShare,
	revokeShare: botApi.revokeShare,
	memberRequests: botApi.memberRequests,
	decideMemberRequest: botApi.decideMemberRequest,
	syncDiscord: botApi.syncDiscord,
	updateBotProfile: botApi.updateProfile,
	botList: botApi.list,
};
