// settingsApi — user-scoped（scope:'user'）。src/server/routes/settingsRoutes.ts に対応。
// アカウント設定・Discord/Google 連携・バックアップ。botId は付けない。
//
// 例外: GET /api/status はダッシュボード集計で botId を読む（bot-scoped）→ scope:'bot'。
import { api } from "../client";
import type {
	ApiResponse,
	DiscordSettingsResponse,
	GoogleOAuthUrlResponse,
	StatusResponse,
} from "../types";

const USER = { scope: "user" } as const;
const BOT = { scope: "bot" } as const;

export const settingsApi = {
	/** GET /api/status — ダッシュボード集計（bot-scoped: botId を読む） */
	status: () => api.get<StatusResponse>("/api/status", BOT),

	/** POST /api/settings/profile */
	updateProfile: (body: Record<string, unknown>) =>
		api.post<ApiResponse>("/api/settings/profile", body, USER),
	/** POST /api/settings/delete-account */
	deleteAccount: () =>
		api.post<ApiResponse>("/api/settings/delete-account", {}, USER),

	/** GET /api/settings/user — 現在のユーザー設定（Bot を選ばずに読める。/api/status は bot-scoped） */
	getUserSettings: () =>
		api.get<
			ApiResponse & {
				richReplyEnabled: boolean;
				remindDefaultMinutes: number;
				notifyTarget: { type: string; id: string | null };
				timezone: string;
			}
		>("/api/settings/user", USER),
	/** POST /api/settings/user — ユーザー設定（リマインド既定・通知先等） */
	updateUserSettings: (body: Record<string, unknown>) =>
		api.post<ApiResponse>("/api/settings/user", body, USER),

	/** POST /api/settings/gemini — Gemini API キー/モデル設定 */
	getGemini: () =>
		api.get<
			ApiResponse & {
				provider: string;
				model: string;
				models: string[];
				/** API キーが登録済みか（本体は返さない） */
				hasApiKey?: boolean;
			}
		>("/api/settings/gemini", USER),
	updateGemini: (body: {
		apiKey?: string;
		model?: string;
		provider?: string;
	}) => api.post<ApiResponse>("/api/settings/gemini", body, USER),

	// ── Discord 連携 ──
	// ★bot-scoped: サーバは GET ?botId= / POST body.botId で対象Botを特定し、
	//   オーナー（system_default は Admin）権限を検査する（settingsRoutes.ts:507-）。
	//   USER だと botId 欠落 → 常に system_default 扱い → 非Adminは 403、
	//   独自Botのトークン閲覧/保存が誤対象になる。
	/** GET /api/settings/discord */
	getDiscord: () =>
		api.get<DiscordSettingsResponse>("/api/settings/discord", BOT),
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
	/** GET /api/settings/backup — 現在のバックアップ設定（フォルダ ID はマスクしない） */
	getBackup: () =>
		api.get<
			ApiResponse & {
				enabled: boolean;
				folderId: string | null;
				intervalHours: number;
				generations: number;
				lastRunAt: string | null;
			}
		>("/api/settings/backup", USER),
	/** POST /api/settings/backup — バックアップ設定 */
	updateBackup: (body: Record<string, unknown>) =>
		api.post<ApiResponse>("/api/settings/backup", body, USER),
	/** POST /api/settings/backup/trigger — 手動バックアップ実行 */
	triggerBackup: () =>
		api.post<ApiResponse>("/api/settings/backup/trigger", {}, USER),
	/** 所有する全Botと関連データをPIN暗号化した移行パッケージとして取得 */
	downloadPackage: (pin: string) =>
		api.downloadPost("/api/settings/package/download", { pin }),
};
