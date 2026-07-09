// settingsApi — user-scoped（scope:'user'）。src/server/routes/settingsRoutes.ts に対応。
// アカウント設定・Discord/Google 連携・バックアップ。botId は付けない。
//
// 例外: GET /api/status はダッシュボード集計で botId を読む（bot-scoped）→ scope:'bot'。
import { api } from "../client";
import type {
	StatusResponse,
	DiscordSettingsResponse,
	GoogleOAuthUrlResponse,
	ApiResponse,
} from "../types";

const USER = { scope: "user" } as const;
const BOT = { scope: "bot" } as const;

export const settingsApi = {
	/** GET /api/status — ダッシュボード集計（bot-scoped: botId を読む） */
	status: () => api.get<StatusResponse>("/api/status", BOT),

	/** POST /api/settings/profile */
	updateProfile: (body: Record<string, unknown>) =>
		api.post<ApiResponse>("/api/settings/profile", body, USER),
	/** POST /api/settings/password */
	changePassword: (body: { currentPassword: string; newPassword: string }) =>
		api.post<ApiResponse>("/api/settings/password", body, USER),
	/** POST /api/settings/delete-account */
	deleteAccount: (body: { password: string }) =>
		api.post<ApiResponse>("/api/settings/delete-account", body, USER),

	/** POST /api/settings/user — ユーザー設定（リマインド既定・通知先等） */
	updateUserSettings: (body: Record<string, unknown>) =>
		api.post<ApiResponse>("/api/settings/user", body, USER),

	/** POST /api/settings/gemini — Gemini API キー/モデル設定 */
	updateGemini: (body: { apiKey?: string; model?: string }) =>
		api.post<ApiResponse>("/api/settings/gemini", body, USER),

	// ── Discord 連携 ──
	// ★bot-scoped: サーバは GET ?botId= / POST body.botId で対象Botを特定し、
	//   オーナー（system_default は Admin）権限を検査する（settingsRoutes.ts:507-）。
	//   USER だと botId 欠落 → 常に system_default 扱い → 非Adminは 403、
	//   独自Botのトークン閲覧/保存が誤対象になる。
	/** GET /api/settings/discord */
	getDiscord: () => api.get<DiscordSettingsResponse>("/api/settings/discord", BOT),
	/** POST /api/settings/discord */
	updateDiscord: (body: Record<string, unknown>) =>
		api.post<ApiResponse>("/api/settings/discord", body, BOT),

	// ── Google 連携 ──
	/** GET /api/settings/google/oauth/url — OAuth 開始 URL 取得 */
	googleOAuthUrl: () =>
		api.get<GoogleOAuthUrlResponse>("/api/settings/google/oauth/url", USER),
	// GET /api/settings/google/oauth/callback はリダイレクト応答（ブラウザ遷移）のため
	// fetch 経由では叩かない（ここでは提供しない）。
	/** POST /api/settings/calendars — 同期対象カレンダー設定 */
	updateCalendars: (body: { calendars: string[]; calendarId?: string }) =>
		api.post<ApiResponse>("/api/settings/calendars", body, USER),

	// ── バックアップ ──
	/** POST /api/settings/backup — バックアップ設定 */
	updateBackup: (body: Record<string, unknown>) =>
		api.post<ApiResponse>("/api/settings/backup", body, USER),
	/** POST /api/settings/backup/trigger — 手動バックアップ実行 */
	triggerBackup: () => api.post<ApiResponse>("/api/settings/backup/trigger", {}, USER),
};
