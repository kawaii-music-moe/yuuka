// botApi — user-scoped（scope:'user'）。src/server/routes/botRoutes.ts / botAttributeRoutes.ts に対応。
//
// ★重要: /api/bots 配下は「プレフィックス一致」で botId 自動注入から除外される。
//   これらは自前の body/query で botId を明示的に運ぶため、自動注入で上書きしてはならない。
//   → 全メソッド scope:'user'。botId が必要なものは呼び出し側が明示 query/body で渡す。
import { api } from "../client";
import type {
	ApiResponse,
	BotListResponse,
	BotResponse,
	BotSharesResponse,
	BotUsageResponse,
	PresetsResponse,
} from "../types";

const USER = { scope: "user" } as const;

export const botApi = {
	/** GET /api/bots — 所有＋共有 Bot 一覧 */
	list: () => api.get<BotListResponse>("/api/bots", USER),

	/** POST /api/bots — Bot 作成 */
	create: (body: {
		name: string;
		preset: string;
		token?: string;
		geminiApiKey?: string;
	}) => api.post<BotResponse>("/api/bots", body, USER),

	/** DELETE /api/bots — Bot 削除（対象 botId は body/query で明示） */
	remove: (botId: string) =>
		api.del<ApiResponse>("/api/bots", { ...USER, query: { botId } }),

	/** POST /api/bots/sync-discord — Discord プロフィール同期 */
	syncDiscord: (botId: string) =>
		api.post<BotResponse>("/api/bots/sync-discord", { botId }, USER),

	/** POST /api/bots/profile — トークン/APIキー等プロフィール更新 */
	updateProfile: (body: Record<string, unknown>) =>
		api.post<BotResponse>("/api/bots/profile", body, USER),

	/** GET /api/bots/presets — プリセット一覧 */
	presets: () => api.get<PresetsResponse>("/api/bots/presets", USER),

	/**
	 * GET /api/bots/usage — 使用量。呼び出し側が手動で botId を query 付与する
	 * （§10.1・現 app.js:1806。scope:'user' なので自動注入されない）。
	 */
	usage: (botId: string) =>
		api.get<BotUsageResponse>("/api/bots/usage", { ...USER, query: { botId } }),

	// ── 共有（shares） ──
	/** GET /api/bots/shares */
	shares: (botId: string) =>
		api.get<BotSharesResponse>("/api/bots/shares", {
			...USER,
			query: { botId },
		}),
	/** POST /api/bots/shares/invite */
	inviteShare: (body: { botId: string; granteeUsername: string }) =>
		api.post<ApiResponse>("/api/bots/shares/invite", body, USER),
	/** POST /api/bots/shares/revoke */
	revokeShare: (body: { botId: string; granteeId: string }) =>
		api.post<ApiResponse>("/api/bots/shares/revoke", body, USER),

	// ── メンバーリクエスト（memberRequestRoutes: /api/bots/member-requests 配下） ──
	/** POST /api/bots/member-requests — 参加リクエスト送信 */
	requestMembership: (body: { botId: string; message?: string }) =>
		api.post<ApiResponse>("/api/bots/member-requests", body, USER),
	/** GET /api/bots/member-requests/mine — 自分の申請一覧 */
	myMemberRequests: () =>
		api.get<ApiResponse>("/api/bots/member-requests/mine", USER),
	/** GET /api/bots/member-requests?botId= — 受信した申請一覧（オーナー向け） */
	memberRequests: (botId: string) =>
		api.get<ApiResponse>("/api/bots/member-requests", {
			...USER,
			query: { botId },
		}),
	/** POST /api/bots/member-requests/:id/decide */
	decideMemberRequest: (id: number, body: { approve: boolean }) =>
		api.post<ApiResponse>(`/api/bots/member-requests/${id}/decide`, body, USER),
};
