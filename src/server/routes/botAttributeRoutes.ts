import { getGuildOptionsForBot } from "../../bot.js";
import { addAuditLog } from "../../db/auditRepo.js";
import {
	addAllowedGuild,
	addAllowedRole,
	addBotMember,
	listAllowedGuilds,
	listAllowedRoles,
	listBotMembers,
	removeAllowedGuild,
	removeAllowedRole,
	removeBotMember,
} from "../../db/botAttributesRepo.js";
import {
	BOT_NOTE_MAX_LENGTH,
	getBotGuildNote,
	setBotGuildNote,
} from "../../db/botNoteRepo.js";
import {
	type BotRecord,
	getBotById,
	hasBotAccess,
	setBotPersona,
	updateBotGeminiKey,
} from "../../db/botRepo.js";
import {
	isKnownSelectableModule,
	listSelectableModules,
} from "../../functions/index.js";
import { listServersGrantedToBot } from "../../db/mcpRepo.js";
import {
	countBotDailyUsage,
	getBotUsageSeries,
} from "../../db/messageLogRepo.js";
import {
	getPersonaById,
	listPersonasForUser,
	listPublicPersonas,
} from "../../db/personaRepo.js";
import { setSystemSetting } from "../../db/systemSettingsRepo.js";
import { isAdmin } from "../../db/userRepo.js";
import {
	applyBotPreset,
	BOT_PRESETS,
	type BotPresetId,
	listPresets,
	parseCapabilities,
	presetIdForCapabilities,
	resolveBotCapabilities,
	setPresetDisplayName,
} from "../../services/botCapabilities.js";
import {
	getUserModulesJson,
	setUserModules,
} from "../../db/botUserModulesRepo.js";
import {
	invalidateUserModulesCache,
	parseEnabledModules,
} from "../../services/botModules.js";
import {
	getRateLimitSettings,
	RATE_LIMIT_DEFAULTS,
} from "../../services/botRateLimit.js";
import type { RouteDef, RouteRequestCtx } from "../../types/contracts.js";
import { sendJson } from "../../types/contracts.js";
import { encryptText } from "../../utils/crypto.js";

// ─── Bot属性・汎用モード設定 HTTPルート（bot_attributes_requirements.md §4.7） ─

/** DiscordのID形式（ギルドID・ユーザーID）の簡易検証 */
function isSnowflake(value: string): boolean {
	return /^\d{5,25}$/.test(value);
}

/** Gemini APIキーの形式検証（接頭辞は問わず英数記号 20〜256 文字。誤値保存の防止用） */
export function isLikelyGeminiKey(value: string): boolean {
	return /^[0-9A-Za-z_.-]{20,256}$/.test(value);
}

/**
 * リクエストから botId を解決し、owner（または Admin）であることを検証する。
 * 失敗時はレスポンス送信済みのため null を返す（要件 §6: 設定変更は owner / Admin のみ）。
 */
function requireOwnedBot(ctx: RouteRequestCtx): BotRecord | null {
	const botId =
		(typeof ctx.body.botId === "string" && ctx.body.botId) ||
		ctx.url.searchParams.get("botId") ||
		"";
	if (!botId) {
		sendJson(ctx.res, 400, { success: false, message: "botId が必要です。" });
		return null;
	}
	const bot = getBotById(botId);
	if (!bot) {
		sendJson(ctx.res, 404, {
			success: false,
			message: "Botが見つかりません。",
		});
		return null;
	}
	if (bot.user_id !== ctx.user!.discordId && !isAdmin(ctx.user!.discordId)) {
		sendJson(ctx.res, 403, {
			success: false,
			message: "Botの作成者のみが設定を変更できます。",
		});
		return null;
	}
	return bot;
}

export const botAttributeRoutes: RouteDef[] = [
	// ── プリセット一覧（Bot作成フォーム用。表示名はシステム設定を反映 §3.3） ──
	{
		method: "GET",
		path: "/api/bots/presets",
		auth: "user",
		async handler(ctx) {
			sendJson(ctx.res, 200, { success: true, presets: listPresets() });
		},
	},

	// ── API使用量サマリ（汎用モードのダッシュボード用。読み取り専用・閲覧は hasBotAccess） ──
	// 秘書業務統計(/api/status)と同様、選択中Botのスコープで集計する。書き込みは無いため owner 限定にしない。
	{
		method: "GET",
		path: "/api/bots/usage",
		auth: "user",
		async handler(ctx) {
			const userId = ctx.user!.discordId;
			const rawBotId =
				(typeof ctx.body.botId === "string" && ctx.body.botId) ||
				ctx.url.searchParams.get("botId") ||
				"";
			// アクセス権の無いBotは system_default にフォールバック（他人のBotの利用量を覗けないようにする）
			const botId =
				rawBotId && hasBotAccess(userId, rawBotId)
					? rawBotId
					: "system_default";

			const daysRaw = parseInt(ctx.url.searchParams.get("days") || "14", 10);
			const days =
				Number.isFinite(daysRaw) && daysRaw > 0 ? Math.min(daysRaw, 90) : 14;

			const { series, totals } = getBotUsageSeries(botId, days);
			sendJson(ctx.res, 200, {
				success: true,
				days,
				series,
				totals,
				rate_limits: getRateLimitSettings(),
			});
		},
	},

	// ── Bot属性（プリセット）の変更（owner / Admin のみ。即時反映 §4.1） ──
	{
		method: "POST",
		path: "/api/bots/attributes",
		auth: "user",
		async handler(ctx) {
			const bot = requireOwnedBot(ctx);
			if (!bot) return;
			if (bot.id === "system_default") {
				return sendJson(ctx.res, 400, {
					success: false,
					message: "デフォルトBotの属性は変更できません。",
				});
			}

			const presetInput =
				typeof ctx.body.preset === "string" ? ctx.body.preset : "";
			if (!(presetInput in BOT_PRESETS)) {
				return sendJson(ctx.res, 400, {
					success: false,
					message: "不明なプリセットです。",
				});
			}
			const preset = presetInput as BotPresetId;

			const ok = applyBotPreset(bot.id, preset);
			if (ok) {
				// 属性変更は監査ログへ記録する（要件 §4.1）
				addAuditLog(
					ctx.user!.discordId,
					"bot.capabilities_change",
					bot.id,
					preset,
				);
			}
			sendJson(ctx.res, 200, {
				success: ok,
				message: ok
					? `Botの属性を変更しました（次のメッセージ処理から反映されます）。${preset === "mcp_assistant" ? " 汎用モードの応答にはBot専用のGemini APIキー・応答許可ギルドの設定が必要です。" : ""}`
					: "属性の変更に失敗しました。",
			});
		},
	},

	// ── 有効モジュール: 取得（function_modularization.md §4改訂。ユーザー個別。アクセス権のある全ユーザー） ──
	// 当該Botの capability 配下の selectable モジュールと、発話ユーザー視点の有効/無効状態を返す。
	// 解決: ユーザー上書き(bot_user_modules) → Bot既定(bots.enabled_modules) → 全有効。
	{
		method: "GET",
		path: "/api/bots/modules",
		auth: "user",
		async handler(ctx) {
			const userId = ctx.user!.discordId;
			const botId =
				(typeof ctx.body.botId === "string" && ctx.body.botId) ||
				ctx.url.searchParams.get("botId") ||
				"";
			if (!botId || !hasBotAccess(userId, botId)) {
				return sendJson(ctx.res, 404, {
					success: false,
					message: "Botが見つかりません。",
				});
			}
			const bot = getBotById(botId);
			const caps = resolveBotCapabilities(botId);
			const overrideJson = getUserModulesJson(botId, userId);
			const hasOverride = overrideJson != null;
			// override があればそれを、無ければ Bot既定を採用（どちらも null = 全有効）。
			const enabled = hasOverride
				? (parseEnabledModules(overrideJson) ?? new Set<string>())
				: parseEnabledModules(bot?.enabled_modules);
			const modules = listSelectableModules()
				.filter((m) => m.cap === "core" || caps.has(m.cap))
				.map((m) => ({
					...m,
					enabled: enabled == null ? true : enabled.has(m.id),
				}));
			sendJson(ctx.res, 200, {
				success: true,
				// 個別上書きの有無。false = Bot既定に従っている（UIの「既定に従う」表示用）。
				has_override: hasOverride,
				all_enabled: enabled == null,
				modules,
			});
		},
	},

	// ── 有効モジュール: 更新（ユーザー個別。アクセス権のある全ユーザー。即時反映＝次メッセージから） ──
	{
		method: "POST",
		path: "/api/bots/modules",
		auth: "user",
		async handler(ctx) {
			const userId = ctx.user!.discordId;
			const botId =
				(typeof ctx.body.botId === "string" && ctx.body.botId) || "";
			if (!botId || !hasBotAccess(userId, botId)) {
				return sendJson(ctx.res, 404, {
					success: false,
					message: "Botが見つかりません。",
				});
			}
			const raw = ctx.body.enabledModules;
			// null（または明示の "all"）= 個別上書きを解除し Bot既定へフォールバック。
			if (raw === null || raw === "all") {
				setUserModules(botId, userId, null);
				invalidateUserModulesCache(botId, userId);
				return sendJson(ctx.res, 200, {
					success: true,
					message: "個別設定を解除し、既定に戻しました（次のメッセージ処理から反映されます）。",
				});
			}
			if (!Array.isArray(raw)) {
				return sendJson(ctx.res, 400, {
					success: false,
					message: "enabledModules は配列または null が必要です。",
				});
			}
			const caps = resolveBotCapabilities(botId);
			const selectable = listSelectableModules();
			// 既知の selectable かつ当該Botの capability 配下のIDのみ採用（重複排除）。
			const accepted = [
				...new Set(
					raw
						.map(String)
						.filter((id) => isKnownSelectableModule(id))
						.filter((id) => {
							const meta = selectable.find((m) => m.id === id);
							return meta != null && (meta.cap === "core" || caps.has(meta.cap));
						}),
				),
			];
			setUserModules(botId, userId, accepted);
			invalidateUserModulesCache(botId, userId);
			sendJson(ctx.res, 200, {
				success: true,
				enabledModules: accepted,
				message: "有効な機能を更新しました（次のメッセージ処理から反映されます）。",
			});
		},
	},

	// ── 汎用モード設定タブの一括取得（owner / Admin のみ §4.7） ──
	{
		method: "GET",
		path: "/api/bots/assistant-config",
		auth: "user",
		async handler(ctx) {
			const bot = requireOwnedBot(ctx);
			if (!bot) return;

			const caps = parseCapabilities(bot.capabilities);
			const ownPersonas = listPersonasForUser(bot.user_id).map((p) => ({
				id: p.id,
				name: p.name,
				scope: "own" as const,
			}));
			const ownIds = new Set(ownPersonas.map((p) => p.id));
			const publicPersonas = listPublicPersonas()
				.filter((p) => !ownIds.has(p.id))
				.map((p) => ({
					id: p.id,
					name: `${p.name}（公開: ${p.owner_username}）`,
					scope: "public" as const,
				}));

			// MCPサーバー: v5では当該Botに利用許可(bot_mcp_access)されたサーバー + システムレベル（常時利用可）。
			// 登録・許可付与は統合管理ページで行うため、ここは読み取り表示のみ。
			const grantedServers = listServersGrantedToBot(bot.id).map((s) => ({
				id: s.id,
				name: s.name,
				enabled: s.enabled === 1,
				system: s.user_id === null,
			}));

			sendJson(ctx.res, 200, {
				success: true,
				preset: presetIdForCapabilities(caps),
				capabilities: Array.from(caps),
				has_gemini_key: !!(
					bot.gemini_api_key_encrypted &&
					bot.gemini_api_key_iv &&
					bot.gemini_api_key_tag
				),
				has_discord_token: !!(
					bot.discord_token_encrypted &&
					bot.discord_token_iv &&
					bot.discord_token_tag
				),
				persona_id: bot.persona_id,
				personas: [...ownPersonas, ...publicPersonas],
				mcp_servers: grantedServers,
				guilds: listAllowedGuilds(bot.id),
				members: listBotMembers(bot.id),
				roles: listAllowedRoles(bot.id),
				usage: countBotDailyUsage(bot.id, 14),
				rate_limits: getRateLimitSettings(),
			});
		},
	},

	// ── Bot専用 Gemini APIキー（汎用モードで設定必須 §4.3.3） ──
	{
		method: "POST",
		path: "/api/bots/assistant/gemini-key",
		auth: "user",
		async handler(ctx) {
			const bot = requireOwnedBot(ctx);
			if (!bot) return;

			const apiKey = typeof ctx.body.apiKey === "string" ? ctx.body.apiKey : "";

			if (!apiKey.trim()) {
				// 空欄はクリア（キー未設定のBotは応答を停止し、UIに警告が表示される）
				updateBotGeminiKey(bot.id, null, null, null);
				addAuditLog(
					ctx.user!.discordId,
					"bot.gemini_key_change",
					bot.id,
					"cleared",
				);
				return sendJson(ctx.res, 200, {
					success: true,
					message:
						"Bot専用APIキーを削除しました。キーが設定されるまでこのBotは応答しません。",
				});
			}

			if (apiKey.startsWith("••••")) {
				return sendJson(ctx.res, 200, {
					success: true,
					message: "APIキーは変更されていません。",
				});
			}

			// キー形式の検証（誤った値の保存を防ぐ）
			if (!isLikelyGeminiKey(apiKey.trim())) {
				return sendJson(ctx.res, 400, {
					success: false,
					message:
						"Gemini APIキーの形式が正しくありません。Google AI Studio で取得したキーをそのまま入力してください。",
				});
			}

			const enc = encryptText(apiKey.trim());
			updateBotGeminiKey(bot.id, enc.encrypted, enc.iv, enc.authTag);
			addAuditLog(
				ctx.user!.discordId,
				"bot.gemini_key_change",
				bot.id,
				"updated",
			);
			sendJson(ctx.res, 200, {
				success: true,
				message: "Bot専用のGemini APIキーを保存しました。",
			});
		},
	},

	// ── Bot単位ペルソナ（管理ページからのみ変更可能 §4.4） ──
	{
		method: "POST",
		path: "/api/bots/assistant/persona",
		auth: "user",
		async handler(ctx) {
			const bot = requireOwnedBot(ctx);
			if (!bot) return;

			const personaIdRaw = ctx.body.personaId;
			let personaId: number | null = null;
			if (
				personaIdRaw !== null &&
				personaIdRaw !== undefined &&
				personaIdRaw !== ""
			) {
				personaId = Number(personaIdRaw);
				if (!Number.isInteger(personaId)) {
					return sendJson(ctx.res, 400, {
						success: false,
						message: "personaId が不正です。",
					});
				}
				// 設定可能なのは owner 所有のもの、または公開ペルソナのみ（要件 §4.4）
				const persona = getPersonaById(personaId);
				if (
					!persona ||
					(persona.owner_id !== bot.user_id && persona.is_public !== 1)
				) {
					return sendJson(ctx.res, 403, {
						success: false,
						message:
							"Bot作成者が所有するペルソナ、または公開ペルソナのみ設定できます。",
					});
				}
			}

			const ok = setBotPersona(bot.id, personaId);
			if (ok)
				addAuditLog(
					ctx.user!.discordId,
					"bot.persona_change",
					bot.id,
					personaId === null ? "cleared" : String(personaId),
				);
			sendJson(ctx.res, 200, {
				success: ok,
				message: ok
					? personaId === null
						? "ペルソナ設定を解除しました（デフォルトに戻ります）。"
						: "Botのペルソナを設定しました。"
					: "ペルソナの設定に失敗しました。",
			});
		},
	},

	// ── 応答許可ギルドの管理（bot_guilds §4.3.3 / §6） ──
	{
		method: "POST",
		path: "/api/bots/assistant/guilds",
		auth: "user",
		async handler(ctx) {
			const bot = requireOwnedBot(ctx);
			if (!bot) return;

			const guildId =
				typeof ctx.body.guildId === "string" ? ctx.body.guildId.trim() : "";
			const action = ctx.body.action === "remove" ? "remove" : "add";
			if (!isSnowflake(guildId)) {
				return sendJson(ctx.res, 400, {
					success: false,
					message: "ギルドID（数字）を入力してください。",
				});
			}

			const ok =
				action === "add"
					? addAllowedGuild(bot.id, guildId)
					: removeAllowedGuild(bot.id, guildId);
			addAuditLog(
				ctx.user!.discordId,
				"bot.guild_allow_change",
				bot.id,
				`${guildId}:${action}`,
			);
			sendJson(ctx.res, 200, {
				success: true,
				guilds: listAllowedGuilds(bot.id),
				message: ok
					? action === "add"
						? "応答許可ギルドへ追加しました。"
						: "応答許可ギルドから削除しました。"
					: "変更はありませんでした。",
			});
		},
	},

	// ── 利用メンバーの管理（owner は管理UIから追加・削除 §4.3.3） ──
	{
		method: "POST",
		path: "/api/bots/assistant/members",
		auth: "user",
		async handler(ctx) {
			const bot = requireOwnedBot(ctx);
			if (!bot) return;

			const guildId =
				typeof ctx.body.guildId === "string" ? ctx.body.guildId.trim() : "";
			const userId =
				typeof ctx.body.userId === "string" ? ctx.body.userId.trim() : "";
			const action = ctx.body.action === "remove" ? "remove" : "add";
			if (!isSnowflake(guildId) || !isSnowflake(userId)) {
				return sendJson(ctx.res, 400, {
					success: false,
					message: "ギルドIDとユーザーID（数字）を入力してください。",
				});
			}

			const ok =
				action === "add"
					? addBotMember(bot.id, guildId, userId, ctx.user!.discordId)
					: removeBotMember(bot.id, guildId, userId);
			addAuditLog(
				ctx.user!.discordId,
				action === "add" ? "bot.member_add" : "bot.member_remove",
				`${bot.id}:${guildId}:${userId}`,
			);
			sendJson(ctx.res, 200, {
				success: true,
				members: listBotMembers(bot.id),
				message: ok
					? action === "add"
						? "利用メンバーへ追加しました。"
						: "利用メンバーから削除しました。"
					: "変更はありませんでした。",
			});
		},
	},

	// ── 利用可能ロールの管理（許可ロール保有者を利用メンバー扱い） ──
	{
		method: "POST",
		path: "/api/bots/assistant/roles",
		auth: "user",
		async handler(ctx) {
			const bot = requireOwnedBot(ctx);
			if (!bot) return;

			const guildId =
				typeof ctx.body.guildId === "string" ? ctx.body.guildId.trim() : "";
			const roleId =
				typeof ctx.body.roleId === "string" ? ctx.body.roleId.trim() : "";
			const roleName =
				typeof ctx.body.roleName === "string" ? ctx.body.roleName : undefined;
			const action = ctx.body.action === "remove" ? "remove" : "add";
			if (!isSnowflake(guildId) || !isSnowflake(roleId)) {
				return sendJson(ctx.res, 400, {
					success: false,
					message: "ギルドIDとロールID（数字）を入力してください。",
				});
			}

			const ok =
				action === "add"
					? addAllowedRole(
							bot.id,
							guildId,
							roleId,
							ctx.user!.discordId,
							roleName,
						)
					: removeAllowedRole(bot.id, guildId, roleId);
			addAuditLog(
				ctx.user!.discordId,
				action === "add" ? "bot.role_add" : "bot.role_remove",
				`${bot.id}:${guildId}:${roleId}`,
			);
			sendJson(ctx.res, 200, {
				success: true,
				roles: listAllowedRoles(bot.id),
				message: ok
					? action === "add"
						? "利用可能ロールへ追加しました。"
						: "利用可能ロールから削除しました。"
					: "変更はありませんでした。",
			});
		},
	},

	// ── プルダウン用: ギルドのロール/メンバー候補（owner / Admin のみ） ──
	{
		method: "GET",
		path: "/api/bots/assistant/guild-options",
		auth: "user",
		async handler(ctx) {
			const bot = requireOwnedBot(ctx);
			if (!bot) return;
			const guildId = ctx.url.searchParams.get("guildId") || "";
			if (!isSnowflake(guildId)) {
				return sendJson(ctx.res, 400, {
					success: false,
					message: "guildId が必要です。",
				});
			}
			const options = await getGuildOptionsForBot(bot.id, guildId);
			sendJson(ctx.res, 200, { success: true, ...options });
		},
	},

	// ── 共有ノート（ギルドノート）の閲覧・編集（owner §4.7） ──
	{
		method: "GET",
		path: "/api/bots/assistant/guild-note",
		auth: "user",
		async handler(ctx) {
			const bot = requireOwnedBot(ctx);
			if (!bot) return;
			const guildId = ctx.url.searchParams.get("guildId") || "";
			if (!isSnowflake(guildId)) {
				return sendJson(ctx.res, 400, {
					success: false,
					message: "guildId が必要です。",
				});
			}
			const content = getBotGuildNote(bot.id, guildId);
			sendJson(ctx.res, 200, {
				success: true,
				content,
				max_length: BOT_NOTE_MAX_LENGTH,
			});
		},
	},
	{
		method: "POST",
		path: "/api/bots/assistant/guild-note",
		auth: "user",
		async handler(ctx) {
			const bot = requireOwnedBot(ctx);
			if (!bot) return;
			const guildId =
				typeof ctx.body.guildId === "string" ? ctx.body.guildId.trim() : "";
			const content =
				typeof ctx.body.content === "string" ? ctx.body.content : "";
			if (!isSnowflake(guildId)) {
				return sendJson(ctx.res, 400, {
					success: false,
					message: "guildId が必要です。",
				});
			}
			// バリデーションはハンドラ内で明示的に行いユーザー向けメッセージを返す。
			// 想定外（DB/IO）の例外はサーバーログに留め、内部詳細をクライアントへ漏らさない。
			if (content.length > BOT_NOTE_MAX_LENGTH) {
				return sendJson(ctx.res, 400, {
					success: false,
					message: `共有ノートは${BOT_NOTE_MAX_LENGTH.toLocaleString()}文字以内です（現在: ${content.length.toLocaleString()}文字）`,
				});
			}
			try {
				setBotGuildNote(bot.id, guildId, content);
				sendJson(ctx.res, 200, {
					success: true,
					message: "共有ノートを保存しました。",
				});
			} catch (err) {
				console.error("[guild-note] 保存エラー:", err);
				sendJson(ctx.res, 500, {
					success: false,
					message: "共有ノートの保存に失敗しました。",
				});
			}
		},
	},

	// ── Admin: プリセット表示名・レート制限既定値（§3.3 / §6。変更権限は Admin） ──
	{
		method: "GET",
		path: "/api/admin/bot-attribute-settings",
		auth: "admin",
		async handler(ctx) {
			sendJson(ctx.res, 200, {
				success: true,
				presets: listPresets(),
				rate_limits: getRateLimitSettings(),
			});
		},
	},
	{
		method: "POST",
		path: "/api/admin/bot-attribute-settings",
		auth: "admin",
		async handler(ctx) {
			const displayNames = (ctx.body.displayNames ?? {}) as Record<
				string,
				unknown
			>;
			for (const presetId of Object.keys(BOT_PRESETS) as BotPresetId[]) {
				const name = displayNames[presetId];
				if (typeof name === "string" && name.trim()) {
					setPresetDisplayName(presetId, name);
				}
			}

			const rateLimits = (ctx.body.rateLimits ?? {}) as Record<string, unknown>;
			const limitEntries: Array<[keyof typeof RATE_LIMIT_DEFAULTS, unknown]> = [
				["userPerMinute", rateLimits.userPerMinute],
				["userPerDay", rateLimits.userPerDay],
				["guildPerDay", rateLimits.guildPerDay],
			];
			for (const [name, value] of limitEntries) {
				const parsed = Number(value);
				if (Number.isInteger(parsed) && parsed > 0) {
					setSystemSetting(RATE_LIMIT_DEFAULTS[name].key, String(parsed));
				}
			}

			addAuditLog(ctx.user!.discordId, "admin.bot_attribute_settings_change");
			sendJson(ctx.res, 200, {
				success: true,
				presets: listPresets(),
				rate_limits: getRateLimitSettings(),
				message: "Bot属性の設定を保存しました。",
			});
		},
	},
];
