import { google } from "googleapis";
import { restartDefaultBot, startCustomBot, stopCustomBot } from "../../bot.js";
import { config } from "../../config.js";
import { addAuditLog } from "../../db/auditRepo.js";
import {
	getBotById,
	getBotDiscordConfig,
	hasBotAccess,
	isBotSuspended,
	listAllBots,
	updateBotDiscordToken,
} from "../../db/botRepo.js";
import { getDb } from "../../db/database.js";
import { revokeAllDesktopTokensForUser } from "../../db/desktopTokenRepo.js";
import {
	addGoogleAccount,
	getPrimaryGoogleAccount,
	listGoogleAccountsSafe,
} from "../../db/googleAccountRepo.js";
import { getActivePersonaIdForBot } from "../../db/personaRepo.js";
import {
	countAdmins,
	deleteUser,
	getUserBackupConfig,
	getUserByDiscordId,
	getUserGeminiConfig,
	getUserNotifyTarget,
	getUserRemindDefaultMinutes,
	getUserRichReplyEnabled,
	isAdmin,
	updatePassword,
	updateUserBackupSettings,
	updateUserGeminiSettings,
	updateUserGoogleSettings,
	updateUsername,
	updateUserSettings,
	verifyPassword,
} from "../../db/userRepo.js";
import {
	getCachedCalendars,
	invalidateCalendarCache,
	invalidateCalendarCacheForAccount,
} from "../../services/googleCalendarService.js";
import { extractDriveFolderId } from "../../services/googleDriveService.js";
import { validatePassword } from "../../services/passwordPolicy.js";
import {
	createSession,
	destroyAllSessionsForUser,
	destroySession,
} from "../../services/sessionService.js";
import type { RouteDef, RouteRequestCtx } from "../../types/contracts.js";
import { sendJson } from "../../types/contracts.js";
import { decryptText, encryptText } from "../../utils/crypto.js";
import {
	consumeOAuthState,
	createOAuthState,
} from "../../utils/oauthStateStore.js";
import { getSessionToken, setSessionCookie } from "../httpHelpers.js";
import { isLikelyGeminiKey } from "./botAttributeRoutes.js";

// ─── ユーザー設定・ステータス HTTPルート ─────────────────────────────────────

/**
 * OAuthリダイレクトURIの構築。
 * セキュリティ: redirect_uri はセキュリティ上重要な絶対URLのため、設定済みの BASE_URL からのみ導出する。
 * BASE_URL 未設定時はローカル開発（localhost/127.0.0.1）に限り Host から構築し、それ以外は拒否する
 * （攻撃者が操作可能な Host / X-Forwarded-Proto ヘッダから redirect_uri を作らせない）。
 */
function buildOAuthRedirectUri(ctx: RouteRequestCtx): string {
	if (config.baseUrl) {
		return `${config.baseUrl.replace(/\/$/, "")}/api/settings/google/oauth/callback`;
	}
	const host = (ctx.req.headers.host || "").toLowerCase();
	const hostname = host.split(":")[0];
	if (hostname === "localhost" || hostname === "127.0.0.1") {
		return `http://${host}/api/settings/google/oauth/callback`;
	}
	throw new Error(
		"OAuth リダイレクトURIを構築できません。環境変数 BASE_URL を設定してください。",
	);
}

/** クレデンシャルの安全なマスキング */
function mask(str: string | null): string {
	if (!str) return "未設定";
	if (str.length <= 8) return "****";
	return str.substring(0, 4) + "..." + str.substring(str.length - 4);
}

/** システムに Google OAuth2 のクライアント設定（ID/Secret）が登録されているか。 */
function isGoogleOAuthConfigured(): boolean {
	return !!(config.googleClientId && config.googleClientSecret);
}

/** 認可コードフロー用の OAuth2 クライアント（認可URL生成・トークン交換で共用）。 */
function buildInteractiveOAuthClient(ctx: RouteRequestCtx) {
	return new google.auth.OAuth2(
		config.googleClientId,
		config.googleClientSecret,
		buildOAuthRedirectUri(ctx),
	);
}

/** 認可URL生成時に要求する OAuth スコープ（同意画面・付与権限を規定）。 */
const GOOGLE_OAUTH_SCOPES = [
	"openid",
	"https://www.googleapis.com/auth/userinfo.email",
	"https://www.googleapis.com/auth/userinfo.profile",
	"https://www.googleapis.com/auth/calendar",
	"https://www.googleapis.com/auth/drive.file",
];

export const settingsRoutes: RouteDef[] = [
	// ── システムステータス（ユーザー個別のダッシュボード用集計） ──
	{
		method: "GET",
		path: "/api/status",
		auth: "user",
		async handler(ctx) {
			const userId = ctx.user!.discordId;
			const db = getDb();

			// 秘書業務データのBot別分離（§v3）: ダッシュボードは選択中のBotのスコープで集計する
			const rawBotId = ctx.url.searchParams.get("botId") ?? undefined;
			const botId =
				rawBotId && hasBotAccess(userId, rawBotId)
					? rawBotId
					: "system_default";

			// v3: (user_id, bot_id) スコープの統計（読み取り専用の集計クエリ）
			// v12: 件数は親タスクのみ集計する（サブタスクは parent_id IS NULL でない＝個別カウントしない）。
			const todoCount = db
				.prepare(
					"SELECT COUNT(*) as count FROM todos WHERE user_id = ? AND bot_id = ? AND parent_id IS NULL",
				)
				.get(userId, botId) as { count: number };
			const openTodoCount = db
				.prepare(
					"SELECT COUNT(*) as count FROM todos WHERE user_id = ? AND bot_id = ? AND status = 'open' AND parent_id IS NULL",
				)
				.get(userId, botId) as { count: number };
			const scheduleCount = db
				.prepare(
					"SELECT COUNT(*) as count FROM schedules WHERE user_id = ? AND bot_id = ?",
				)
				.get(userId, botId) as { count: number };
			const expenseCount = db
				.prepare(
					"SELECT COUNT(*) as count FROM expenses WHERE user_id = ? AND bot_id = ?",
				)
				.get(userId, botId) as { count: number };

			// 優先度別の未完了ToDo数（v2: high/medium/low。旧UI互換のため 2/1/0 キーでも返す）
			const priorityRows = db
				.prepare(`
        SELECT priority, COUNT(*) as count
        FROM todos
        WHERE user_id = ? AND bot_id = ? AND status = 'open' AND parent_id IS NULL
        GROUP BY priority
      `)
				.all(userId, botId) as { priority: string | null; count: number }[];

			const priorityMap: Record<number, number> = { 0: 0, 1: 0, 2: 0 };
			for (const row of priorityRows) {
				if (row.priority === "high") priorityMap[2] += row.count;
				else if (row.priority === "medium") priorityMap[1] += row.count;
				else priorityMap[0] += row.count;
			}

			// スケジュールの直近5日間のイベント数推移 (今日から4日後まで)
			const scheduleTrend: number[] = [];
			for (let i = 0; i < 5; i++) {
				const dateStr = new Date(Date.now() + i * 24 * 60 * 60 * 1000)
					.toISOString()
					.slice(0, 10);
				const countRow = db
					.prepare(`
          SELECT COUNT(*) as count
          FROM schedules
          WHERE user_id = ? AND bot_id = ? AND date(start_at) = date(?)
        `)
					.get(userId, botId, dateStr) as { count: number };
				scheduleTrend.push(countRow ? countRow.count : 0);
			}

			// 経費の過去5日間の支出額推移 (4日前から今日まで)
			const expenseTrend: number[] = [];
			for (let i = 4; i >= 0; i--) {
				const dateStr = new Date(Date.now() - i * 24 * 60 * 60 * 1000)
					.toISOString()
					.slice(0, 10);
				const sumRow = db
					.prepare(`
          SELECT SUM(amount) as total
          FROM expenses
          WHERE user_id = ? AND bot_id = ? AND type = 'expense' AND date = ?
        `)
					.get(userId, botId, dateStr) as { total: number | null };
				expenseTrend.push(sumRow && sumRow.total ? sumRow.total : 0);
			}

			const geminiConfig = getUserGeminiConfig(userId);
			const backupConfig = getUserBackupConfig(userId);
			// v5: Google連携状況は user_google_accounts（primary）から判定する。
			const primaryGoogle = getPrimaryGoogleAccount(userId);
			const googleLinked = !!primaryGoogle;

			// 利用可能なカレンダー一覧をフェッチ (連携済みの場合のみ。primaryアカウント基準)
			const calendars = googleLinked ? await getCachedCalendars(userId) : [];

			sendJson(ctx.res, 200, {
				success: true,
				user: {
					discordId: userId,
					username: getUserByDiscordId(userId)?.username || userId,
				},
				stats: {
					tasks: todoCount.count,
					pendingTasks: openTodoCount.count,
					pendingPriorities: priorityMap,
					schedules: scheduleCount.count,
					scheduleTrend,
					expenses: expenseCount.count,
					expenseTrend,
				},
				config: {
					dbPath: config.dbPath,
					reminderCron: config.reminderCron,
					googleCalendarId: primaryGoogle?.calendar_id || "未設定",
					googleCalendars: calendars,
					googleLinked,
					googleAccountCount: listGoogleAccountsSafe(userId).length,
					geminiModel: geminiConfig?.model || "gemini-3.1-flash-lite",
					geminiApiKey: mask(
						geminiConfig?.apiKeyEncrypted ? "configured" : null,
					),
					backupEnabled: backupConfig?.enabled === true,
					backupFolderId: mask(backupConfig?.folderId ?? null),
					backupIntervalHours: backupConfig?.intervalHours ?? 24,
					backupGenerations: backupConfig?.generations ?? 7,
					backupLastRunAt: backupConfig?.lastRunAt ?? null,
					richReplyEnabled: getUserRichReplyEnabled(userId),
					remindDefaultMinutes: getUserRemindDefaultMinutes(userId),
					notifyTarget: getUserNotifyTarget(userId),
					activePersonaId: getActivePersonaIdForBot(userId, botId),
				},
			});
		},
	},

	// ── プロフィール更新 ──
	{
		method: "POST",
		path: "/api/settings/profile",
		auth: "user",
		async handler(ctx) {
			const userId = ctx.user!.discordId;
			const username =
				typeof ctx.body.username === "string" ? ctx.body.username.trim() : "";
			if (!username) {
				return sendJson(ctx.res, 400, {
					success: false,
					message: "有効なユーザーネームを指定してください。",
				});
			}

			const success = updateUsername(userId, username);
			if (!success) {
				return sendJson(ctx.res, 400, {
					success: false,
					message:
						"プロファイルの更新に失敗しました。同じ名前が既に使われている可能性があります。",
				});
			}

			// セッション内のユーザー名を更新するため再発行
			const token = getSessionToken(ctx.req);
			if (token) await destroySession(token);
			const newToken = await createSession({
				discordId: userId,
				username,
				role: ctx.user!.role,
			});
			setSessionCookie(ctx.res, ctx.req, newToken);
			sendJson(ctx.res, 200, {
				success: true,
				message: "プロファイルを更新しました。",
			});
		},
	},

	// ── パスワード変更（§5.4.2: 変更時に全セッション即時失効） ──
	{
		method: "POST",
		path: "/api/settings/password",
		auth: "user",
		async handler(ctx) {
			const userId = ctx.user!.discordId;
			const { currentPassword, newPassword } = ctx.body as Record<
				string,
				string
			>;
			if (!currentPassword || !newPassword) {
				return sendJson(ctx.res, 400, {
					success: false,
					message: "現在のパスワードと新しいパスワードを入力してください。",
				});
			}

			const user = getUserByDiscordId(userId);
			if (!user || !verifyPassword(currentPassword, user.password_hash)) {
				return sendJson(ctx.res, 401, {
					success: false,
					message: "現在のパスワードが正しくありません。",
				});
			}

			const policy = validatePassword(newPassword);
			if (!policy.ok) {
				return sendJson(ctx.res, 400, {
					success: false,
					message:
						policy.reason || "新しいパスワードがポリシーを満たしていません。",
				});
			}

			updatePassword(userId, newPassword);
			await destroyAllSessionsForUser(userId);
			// デスクトップトークンも全失効（backend_api.md §1.4）。各端末は次回 401 で再ログインへ。
			revokeAllDesktopTokensForUser(userId);
			addAuditLog(userId, "auth.password_change");

			// 新しいセッションを発行して継続ログイン
			const newToken = await createSession({
				discordId: userId,
				username: user.username,
				role: ctx.user!.role,
			});
			setSessionCookie(ctx.res, ctx.req, newToken);
			sendJson(ctx.res, 200, {
				success: true,
				message:
					"パスワードを変更しました。他の端末のセッションは無効化されました。",
			});
		},
	},

	// ── アカウント削除（本人による退会。最後の管理者は削除不可） ──
	{
		method: "POST",
		path: "/api/settings/delete-account",
		auth: "user",
		async handler(ctx) {
			const userId = ctx.user!.discordId;
			const { password } = ctx.body as Record<string, string>;
			if (!password) {
				return sendJson(ctx.res, 400, {
					success: false,
					message: "削除を確定するには現在のパスワードを入力してください。",
				});
			}

			// 本人確認: 現在のパスワードを検証する
			const user = getUserByDiscordId(userId);
			if (!user || !verifyPassword(password, user.password_hash)) {
				return sendJson(ctx.res, 401, {
					success: false,
					message: "パスワードが正しくありません。",
				});
			}

			// 管理者ガード: 自分が唯一の管理者なら削除を拒否（管理者0人化を防ぐ）。
			// 先に他のユーザーへ Admin 権限を付与してから再度お試しいただく。
			if (user.role === "admin" && countAdmins() <= 1) {
				return sendJson(ctx.res, 409, {
					success: false,
					message:
						"あなたは唯一の管理者のため、アカウントを削除できません。先に他のユーザーへ管理者権限を付与してください。",
				});
			}

			// 自分が所有する起動中の独自Botを停止（system_default は対象外）
			for (const bot of listAllBots()) {
				if (bot.user_id === userId && bot.id !== "system_default") {
					stopCustomBot(bot.id);
				}
			}

			const ok = deleteUser(userId);
			if (!ok) {
				return sendJson(ctx.res, 404, {
					success: false,
					message: "アカウントが見つかりません。",
				});
			}

			// 全セッションを失効させてログアウトさせる
			await destroyAllSessionsForUser(userId);
			addAuditLog(userId, "auth.account_delete");
			sendJson(ctx.res, 200, {
				success: true,
				message: "アカウントを削除しました。関連データもすべて削除されました。",
			});
		},
	},

	// ── ユーザー設定更新（リッチ返信・リマインド既定・通知先 §3.0.5, §3.3.2） ──
	{
		method: "POST",
		path: "/api/settings/user",
		auth: "user",
		async handler(ctx) {
			const {
				richReplyEnabled,
				remindDefaultMinutes,
				notifyTargetType,
				notifyTargetId,
				timezone,
			} = ctx.body as Record<string, unknown>;

			updateUserSettings(ctx.user!.discordId, {
				...(richReplyEnabled !== undefined
					? { richReplyEnabled: richReplyEnabled === true }
					: {}),
				...(remindDefaultMinutes !== undefined
					? {
							remindDefaultMinutes: Math.max(
								0,
								Number(remindDefaultMinutes) || 0,
							),
						}
					: {}),
				...(notifyTargetType !== undefined
					? {
							notifyTargetType:
								notifyTargetType === "channel"
									? ("channel" as const)
									: ("dm" as const),
						}
					: {}),
				...(notifyTargetId !== undefined
					? {
							notifyTargetId:
								typeof notifyTargetId === "string" && notifyTargetId.trim()
									? notifyTargetId.trim()
									: null,
						}
					: {}),
				...(typeof timezone === "string" && timezone.trim()
					? { timezone: timezone.trim() }
					: {}),
			});

			sendJson(ctx.res, 200, {
				success: true,
				message: "ユーザー設定を更新しました。",
			});
		},
	},

	// ── Gemini 設定更新（v2: ユーザー単位のみ §4.2） ──
	{
		method: "POST",
		path: "/api/settings/gemini",
		auth: "user",
		async handler(ctx) {
			const userId = ctx.user!.discordId;
			const { apiKey, model } = ctx.body as Record<string, string>;
			if (!model)
				return sendJson(ctx.res, 400, {
					success: false,
					message: "モデル名は必須項目です。",
				});

			const current = getUserGeminiConfig(userId);
			let encrypted: string | null;
			let iv: string | null;
			let tag: string | null;

			if (apiKey && !apiKey.startsWith("****")) {
				// キー形式の検証（誤った値の保存を防ぐ）
				if (!isLikelyGeminiKey(apiKey.trim())) {
					return sendJson(ctx.res, 400, {
						success: false,
						message:
							"Gemini APIキーの形式が正しくありません。Google AI Studio で取得したキーをそのまま入力してください。",
					});
				}
				const enc = encryptText(apiKey.trim());
				encrypted = enc.encrypted;
				iv = enc.iv;
				tag = enc.authTag;
			} else {
				encrypted = current?.apiKeyEncrypted ?? null;
				iv = current?.apiKeyIv ?? null;
				tag = current?.apiKeyTag ?? null;
			}

			updateUserGeminiSettings(userId, encrypted, iv, tag, model.trim());
			sendJson(ctx.res, 200, {
				success: true,
				message: "Gemini 設定を更新しました。",
			});
		},
	},

	// ── Discord 独自Botトークン設定（§4.3.1: トークンはBotオーナーのみ） ──
	{
		method: "GET",
		path: "/api/settings/discord",
		auth: "user",
		async handler(ctx) {
			const userId = ctx.user!.discordId;
			const botId = ctx.url.searchParams.get("botId") || "system_default";
			// トークン設定状態はオーナー（system_default は Admin）のみ閲覧可能（POSTと同一の権限）
			if (botId === "system_default") {
				if (!isAdmin(userId)) {
					return sendJson(ctx.res, 403, {
						success: false,
						message: "システムBotのDiscord設定は管理者のみ閲覧できます。",
					});
				}
			} else {
				const bot = getBotById(botId);
				if (!bot || bot.user_id !== userId) {
					return sendJson(ctx.res, 403, {
						success: false,
						message: "Botのトークン設定はオーナーのみが閲覧できます。",
					});
				}
			}
			const current = getBotDiscordConfig(botId);
			const hasToken = !!(
				current?.tokenEncrypted &&
				current?.tokenIv &&
				current?.tokenTag
			);
			sendJson(ctx.res, 200, {
				success: true,
				hasToken,
				tokenMasked: hasToken ? "••••••••••••" : "",
			});
		},
	},
	{
		method: "POST",
		path: "/api/settings/discord",
		auth: "user",
		async handler(ctx) {
			const userId = ctx.user!.discordId;
			const botId =
				typeof ctx.body.botId === "string" && ctx.body.botId
					? ctx.body.botId
					: "system_default";
			const token = typeof ctx.body.token === "string" ? ctx.body.token : "";

			if (botId === "system_default") {
				if (!isAdmin(userId)) {
					return sendJson(ctx.res, 403, {
						success: false,
						message: "システムBotのDiscord設定は管理者のみ変更可能です。",
					});
				}
			} else {
				const bot = getBotById(botId);
				if (!bot || bot.user_id !== userId) {
					return sendJson(ctx.res, 403, {
						success: false,
						message: "Botのトークンはオーナーのみが変更できます。",
					});
				}
			}

			const current = getBotDiscordConfig(botId);
			let encrypted: string | null = null;
			let iv: string | null = null;
			let tag: string | null = null;
			let tokenChanged = false;
			let tokenCleared = false;

			// トークンの処理
			if (!token.trim()) {
				// 空欄の場合はクリア（削除）
				if (current?.tokenEncrypted) {
					tokenChanged = true;
					tokenCleared = true;
				}
			} else if (token.startsWith("••••")) {
				// マスクの場合は変更なし
				encrypted = current?.tokenEncrypted ?? null;
				iv = current?.tokenIv ?? null;
				tag = current?.tokenTag ?? null;
			} else {
				// 新しいトークンが入力された
				const enc = encryptText(token.trim());
				encrypted = enc.encrypted;
				iv = enc.iv;
				tag = enc.authTag;
				tokenChanged = true;
			}

			updateBotDiscordToken(botId, encrypted, iv, tag);
			addAuditLog(
				userId,
				"bot.token_change",
				botId,
				tokenCleared ? "cleared" : tokenChanged ? "updated" : "unchanged",
			);

			// トークンがクリアされた場合、動作中の独自Botを完全にクローズする
			if (tokenCleared) {
				stopCustomBot(botId);
			}

			// トークンが変更された場合、新しいBotを非同期でログイン・動的起動する
			let botStartupMessage = "";
			if (tokenChanged && encrypted !== null) {
				if (isBotSuspended(botId)) {
					botStartupMessage =
						" このBotは管理者により停止処分中のため、起動できません。";
				} else if (botId === "system_default") {
					// デフォルトBotは専用の再起動フロー
					const plainToken = decryptText(encrypted, iv!, tag!);
					const ok = await restartDefaultBot(plainToken);
					if (!ok)
						botStartupMessage =
							" 設定は保存されましたが、デフォルトBotの起動に失敗しました。";
				} else {
					console.log(
						`[Discord Bot] Bot ${botId} の独自Botの再起動を試みます...`,
					);
					const startupSuccess = await startCustomBot(botId);
					if (!startupSuccess) {
						botStartupMessage =
							" 設定は保存されましたが、独自Botの起動に失敗しました。トークンが有効か再度ご確認ください。";
					}
				}
			}

			sendJson(ctx.res, 200, {
				success: true,
				message: `設定を保存しました。${botStartupMessage}`.trim(),
			});
		},
	},

	// ── Google OAuth URL 生成（v2: ユーザー単位連携） ──
	{
		method: "GET",
		path: "/api/settings/google/oauth/url",
		auth: "user",
		async handler(ctx) {
			if (!isGoogleOAuthConfigured()) {
				return sendJson(ctx.res, 400, {
					success: false,
					message:
						"システムに Google OAuth2 設定が登録されていません。システム管理者に問い合わせてください。",
				});
			}

			// redirect_uri 構築失敗（BASE_URL 未設定など）は 500 で明示する。
			let oauth2Client: ReturnType<typeof buildInteractiveOAuthClient>;
			try {
				oauth2Client = buildInteractiveOAuthClient(ctx);
			} catch {
				return sendJson(ctx.res, 500, {
					success: false,
					message:
						"OAuth リダイレクトURIを構築できません。システム管理者に BASE_URL の設定を依頼してください。",
				});
			}

			// CSRF対策: セッションユーザーに束縛した一回限りの state nonce を発行する
			const state = await createOAuthState(ctx.user!.discordId);

			const oauthUrl = oauth2Client.generateAuthUrl({
				access_type: "offline",
				scope: GOOGLE_OAUTH_SCOPES,
				prompt: "consent",
				state,
			});

			sendJson(ctx.res, 200, { success: true, url: oauthUrl });
		},
	},

	// ── Google OAuth コールバック（リダイレクト応答のため auth:"none" + 手動セッション検証） ──
	{
		method: "GET",
		path: "/api/settings/google/oauth/callback",
		auth: "none",
		async handler(ctx) {
			const redirect = (location: string) => {
				ctx.res.writeHead(302, { Location: location });
				ctx.res.end();
			};

			// コールバック時点でのセッション確認
			if (!ctx.user) {
				return redirect("/?oauth=error&msg=unauthorized");
			}
			const userId = ctx.user.discordId;

			if (!isGoogleOAuthConfigured()) {
				return redirect("/?oauth=error&msg=missing_config");
			}

			// CSRF対策: state を検証し、フロー開始時のセッションユーザーと一致することを確認する。
			// （一回限りの nonce。攻撃者が用意した code を被害者セッションに紐付ける攻撃を防ぐ）
			const state = ctx.url.searchParams.get("state") || "";
			const stateUserId = await consumeOAuthState(state);
			if (!stateUserId || stateUserId !== userId) {
				return redirect("/?oauth=error&msg=invalid_state");
			}

			try {
				const code = ctx.url.searchParams.get("code");
				if (!code) {
					return redirect("/?oauth=error&msg=missing_code");
				}
				const oauth2Client = buildInteractiveOAuthClient(ctx);

				const { tokens } = await oauth2Client.getToken(code);
				oauth2Client.setCredentials(tokens);

				let googleEmail: string | null = null;
				try {
					const oauth2 = google.oauth2({ version: "v2", auth: oauth2Client });
					const userInfo = await oauth2.userinfo.get();
					googleEmail = userInfo.data.email || null;
				} catch (e) {
					console.warn("Failed to fetch user email during settings link:", e);
				}

				// v5: 複数アカウント連携。新規アカウント行を追加（同一emailは更新）。
				if (tokens.refresh_token) {
					const acct = addGoogleAccount(userId, {
						email: googleEmail,
						refreshToken: tokens.refresh_token,
						calendarId: googleEmail || null,
					});
					invalidateCalendarCacheForAccount(acct.id);
					addAuditLog(userId, "auth.google_link", googleEmail ?? undefined);
					redirect("/?oauth=success");
				} else {
					// refresh_token が来ない（再同意なし）場合は明示エラー。URL生成側で prompt=consent を付与済み。
					redirect("/?oauth=error&msg=no_refresh_token");
				}
			} catch (err) {
				// 情報漏えい対策: 上流エラーの生メッセージはURLに反映せず、固定コードのみ返す
				console.error("Google OAuth Callback Error:", err);
				redirect("/?oauth=error&msg=token_exchange_failed");
			}
		},
	},

	// ── 利用カレンダーリスト更新（v2: ユーザー単位） ──
	{
		method: "POST",
		path: "/api/settings/calendars",
		auth: "user",
		async handler(ctx) {
			const { calendars } = ctx.body as { calendars?: unknown };
			if (!Array.isArray(calendars)) {
				return sendJson(ctx.res, 400, {
					success: false,
					message: "カレンダーリストは配列形式で指定してください。",
				});
			}
			updateUserGoogleSettings(ctx.user!.discordId, {
				calendars: calendars.map(String),
			});
			invalidateCalendarCache(ctx.user!.discordId);
			sendJson(ctx.res, 200, {
				success: true,
				message: "同期対象カレンダーを更新しました。",
			});
		},
	},

	// ── バックアップ設定（v2: ユーザー単位・間隔/世代数 §8.2） ──
	{
		method: "POST",
		path: "/api/settings/backup",
		auth: "user",
		async handler(ctx) {
			const { enabled, folderId, intervalHours, generations } =
				ctx.body as Record<string, unknown>;

			// フォルダ指定はフォルダID単体・Google DriveのフォルダURLのどちらも受け付ける
			let normalizedFolderId: string | undefined;
			if (typeof folderId === "string" && folderId.trim()) {
				const extracted = extractDriveFolderId(folderId);
				if (!extracted) {
					sendJson(ctx.res, 400, {
						success: false,
						message:
							"バックアップ先フォルダの指定が不正です。フォルダIDまたはGoogle DriveのフォルダURLを入力してください。",
					});
					return;
				}
				normalizedFolderId = extracted;
			}

			updateUserBackupSettings(ctx.user!.discordId, {
				enabled: enabled === true,
				intervalHours: Number(intervalHours) || 24,
				generations: Number(generations) || 7,
				folderId: normalizedFolderId,
			});
			sendJson(ctx.res, 200, {
				success: true,
				message: "バックアップ設定を保存しました。",
			});
		},
	},
	{
		method: "POST",
		path: "/api/settings/backup/trigger",
		auth: "user",
		async handler(ctx) {
			try {
				const { runBackup } = await import("../../services/backupService.js");
				const backupUrl = await runBackup(ctx.user!.discordId);
				addAuditLog(ctx.user!.discordId, "backup.manual_run");
				sendJson(ctx.res, 200, {
					success: true,
					url: backupUrl,
					message: "手動バックアップが完了しました。",
				});
			} catch (err) {
				// 上流（Google Drive API・ファイルシステム・OAuth）の生エラーは内部詳細
				// （ファイルパス・トークン失効理由等）を含むためクライアントへは返さない。
				console.error("手動バックアップ実行エラー:", err);
				sendJson(ctx.res, 500, {
					success: false,
					message:
						"手動バックアップに失敗しました。Google連携とバックアップ先フォルダの設定をご確認ください。",
				});
			}
		},
	},
];
