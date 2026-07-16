// ─────────────────────────────────────────────────────────────────────────────
// Bot設定・Discord連携タブのレスポンス型（DOM 非依存）。
//
// サーバの型（src/server/routes/{botRoutes,botAttributeRoutes,settingsRoutes}.ts）は
// zod ビュー strip 後のトップレベル直置きペイロード。フロントが参照する主要フィールドのみ厚めに定義。
// 共通の $lib/api/types に無い、この2タブ固有の形状をここに集約する（芋づる fetch のためのユーティリティ含む）。
// ─────────────────────────────────────────────────────────────────────────────

import type { Bot } from "$lib/stores/activeBot";

/** system_default / bot_default_* は属性系カードを owner/Admin 限定表示にするための判定。 */
export function isSystemDefaultBot(botId: string | undefined | null): boolean {
	return !botId || botId === "system_default";
}
export function isDefaultBot(botId: string | undefined | null): boolean {
	return (
		!botId || botId === "system_default" || botId.startsWith("bot_default_")
	);
}

/** Bot導入リンクで付与する権限（旧 app.js BOT_INVITE_PERMISSIONS）。 */
export const BOT_INVITE_PERMISSIONS = "117760";

export function botInviteUrl(applicationId: string): string {
	return `https://discord.com/oauth2/authorize?client_id=${encodeURIComponent(applicationId)}&scope=bot&permissions=${BOT_INVITE_PERMISSIONS}`;
}
export function botProfileUrl(applicationId: string): string {
	return `https://discord.com/users/${encodeURIComponent(applicationId)}`;
}

// ── /api/status（設定タブが読む config セクション） ───────────────────────────
export interface StatusConfig {
	geminiModel?: string;
	backupEnabled?: boolean;
	backupFolderId?: string;
	backupIntervalHours?: number;
	backupGenerations?: number;
	backupLastRunAt?: string | null;
	richReplyEnabled?: boolean;
	remindDefaultMinutes?: number;
	notifyTarget?: { type?: string; id?: string } | null;
	timezone?: string;
}
export interface StatusConfigResponse {
	success: boolean;
	message?: string;
	config?: StatusConfig;
}

// ── /api/settings/discord ─────────────────────────────────────────────────────
export interface DiscordTokenResponse {
	success: boolean;
	message?: string;
	tokenMasked?: string;
}

// ── /api/bots（属性カードが現在の Bot を探す） ───────────────────────────────
export interface BotAttrView {
	id: string;
	user_id: string;
	name: string;
	preset?: string;
	preset_display_name?: string;
	discord_username?: string | null;
	discord_application_id?: string | null;
	recommended_persona_id?: number | null;
	[k: string]: unknown;
}
export interface BotListResp {
	success: boolean;
	message?: string;
	bots?: BotAttrView[];
}

// ── /api/bots/presets ─────────────────────────────────────────────────────────
export interface PresetItem {
	id: string;
	displayName: string;
	capabilities: string[];
}
export interface PresetsResp {
	success: boolean;
	message?: string;
	presets?: PresetItem[];
}

// ── /api/bots/modules ─────────────────────────────────────────────────────────
export interface ModuleItem {
	id: string;
	label: string;
	description?: string;
	settingsKey?: string;
	enabled: boolean;
}
export interface ModulesResp {
	success: boolean;
	message?: string;
	modules?: ModuleItem[];
	has_override?: boolean;
	all_enabled?: boolean;
}

// ── /api/bots/assistant-config ────────────────────────────────────────────────
export interface AssistantPersona {
	id: number;
	name: string;
	scope?: "own" | "public";
}
export interface AssistantMcpServer {
	id: number;
	name: string;
	enabled: boolean;
	system: boolean;
}
export interface AssistantGuild {
	guild_id: string;
}
export interface AssistantChannel {
	guild_id: string;
	channel_id: string;
}
export interface AssistantMember {
	guild_id: string;
	user_id: string;
}
export interface AssistantRole {
	guild_id: string;
	role_id: string;
	role_name?: string | null;
}
export interface AssistantUsage {
	date: string;
	count: number;
}
export interface AssistantRateLimits {
	userPerMinute: number;
	userPerDay: number;
	guildPerDay: number;
}
export interface AssistantConfigResp {
	success: boolean;
	message?: string;
	has_gemini_key?: boolean;
	has_discord_token?: boolean;
	persona_id?: number | null;
	personas?: AssistantPersona[];
	mcp_servers?: AssistantMcpServer[];
	guilds?: AssistantGuild[];
	channels?: AssistantChannel[];
	members?: AssistantMember[];
	roles?: AssistantRole[];
	usage?: AssistantUsage[];
	rate_limits?: AssistantRateLimits;
}

// ── /api/bots/assistant/guild-options ─────────────────────────────────────────
export interface GuildOptionItem {
	id: string;
	name: string;
}
export interface GuildOptionsResp {
	success: boolean;
	message?: string;
	available?: boolean;
	members?: GuildOptionItem[];
	roles?: GuildOptionItem[];
}

// ── /api/bots/assistant/guild-note ────────────────────────────────────────────
export interface GuildNoteResp {
	success: boolean;
	message?: string;
	content?: string;
	max_length?: number;
}

// ── /api/bots/member-requests ─────────────────────────────────────────────────
export interface MemberRequest {
	id: number;
	user_id: string;
	guild_id: string;
	note?: string | null;
}
export interface MemberRequestsResp {
	success: boolean;
	message?: string;
	requests?: MemberRequest[];
}

// ── /api/bots/shares ──────────────────────────────────────────────────────────
export interface BotShare {
	shared_user_id: string;
	shared_username?: string | null;
	status: "active" | "pending" | "revoked" | string;
}
export interface BotSharesResp {
	success: boolean;
	message?: string;
	shares?: BotShare[];
	recommended_persona_id?: number | null;
}

// ── /api/credentials（設定タブは service_name / username / url を読む旧形状） ──
export interface CredentialRow {
	service_name?: string;
	serviceName?: string;
	username?: string;
	url?: string;
	updated_at?: string;
	updatedAt?: string;
}
export interface CredentialsResp {
	success: boolean;
	message?: string;
	credentials?: CredentialRow[];
}

/** owner/Admin 判定用のヘルパ。activeBot には user_id が無いため /api/bots の結果と突き合わせる。 */
export function isOwnerOrAdmin(
	bot: BotAttrView | undefined,
	activeUserId: string,
	role: "user" | "admin" | undefined,
): boolean {
	if (role === "admin") return true;
	return !!bot && bot.user_id === activeUserId;
}

/** activeBot ストアの Bot（null 可）から id を安全に取り出す。 */
export function botIdOf(b: Bot): string | undefined {
	return b?.id;
}
