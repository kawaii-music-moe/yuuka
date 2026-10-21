// ─────────────────────────────────────────────────────────────────────────────
// botTabs — 設定ハブ（/bot/settings）構成の単一情報源（2階層化）。
// 「どのタブが設定ハブ配下か」とカード表示定義をここに一元管理し、
// BotShell（パンくず/サイドバーのアクティブ判定/適用範囲バッジ）と BotSettingsHub（一覧）
// の両方がここを参照する（2重リスト化による無言の乖離を防ぐ）。
//
// 構成は docs/design/settings-restructure.md の「Bot設定」。カテゴリの並びはプリセットで
// 変えない。プリセットで使えないカテゴリは消さずに 1 行の案内へ畳む（hubGroups）。
// ─────────────────────────────────────────────────────────────────────────────
import type { SettingsScope } from "$lib/components/ui";
import type { BotTab } from "$lib/router";

/** Bot プリセット識別子（未知値・未設定は secretary 扱い）。 */
export type BotPreset = "secretary" | "mcp_assistant";

/** activeBot からプリセットを正規化（既定 secretary）。 */
export function botPreset(
	bot: { preset?: string } | null | undefined,
): BotPreset {
	return bot?.preset === "mcp_assistant" ? "mcp_assistant" : "secretary";
}

export type SettingsCategoryId = "bot" | "automation" | "discord";

export interface SettingsCategory {
	id: SettingsCategoryId;
	label: string;
	/** プリセットのせいで行が 1 つも出ないときに、行の代わりに出す案内。 */
	unavailableNote?: string;
}

/** 設定ハブのカテゴリ（表示順・プリセットで変えない）。 */
export const SETTINGS_CATEGORIES: SettingsCategory[] = [
	{ id: "bot", label: "このBot" },
	{
		id: "automation",
		label: "自動化",
		unavailableNote: "Playbook・定期配信・Webhook は秘書属性のBotで使えます。",
	},
	{ id: "discord", label: "Discord" },
];

/** ハブの行を出すかの判定材料（呼び出し側が activeBot とセッションから組む）。 */
export interface HubContext {
	preset: BotPreset;
	/** system_default / bot_default_*（名前・属性・共有を持たない Bot）か。 */
	defaultBot: boolean;
	/** system_default（トークンは管理者だけが扱う）か。 */
	systemDefaultBot: boolean;
	isAdmin: boolean;
}

export interface SettingsHubItem {
	tab: BotTab;
	label: string;
	desc: string;
	category: SettingsCategoryId;
	/** 表示プリセット限定（未指定は両方）。 */
	only?: BotPreset;
	/** ページ全体の適用範囲。ページ内で範囲が混ざるときは未指定にして各カードに出す。 */
	scope?: SettingsScope;
	/** プリセット以外の表示条件（未指定は常に表示）。 */
	visible?: (ctx: HubContext) => boolean;
}

/** 設定ハブの行定義（表示順）。 */
export const SETTINGS_HUB_ITEMS: SettingsHubItem[] = [
	{
		tab: "profile",
		label: "基本情報と共有",
		desc: "Botの登録名・属性と、Botを利用できるメンバー",
		category: "bot",
		scope: "bot",
		visible: (ctx) => !ctx.defaultBot,
	},
	{
		tab: "personas",
		label: "人格",
		desc: "口調・性格などのペルソナと、会話履歴のクリア",
		category: "bot",
		scope: "bot",
	},
	{
		tab: "features",
		label: "機能と AI",
		desc: "このBotで使う機能と、応答に使う AI",
		category: "bot",
	},
	{
		tab: "access",
		label: "連携の利用許可",
		desc: "このBotに使わせる MCP サーバー・認証情報・Google アカウント",
		category: "bot",
	},
	{
		tab: "playbooks",
		label: "Playbook",
		desc: "自動化の手順書・定期実行・実行履歴",
		category: "automation",
		only: "secretary",
		scope: "bot",
	},
	{
		tab: "delivery",
		label: "定期配信",
		desc: "朝報と、タスク・支出サマリーの配信",
		category: "automation",
		only: "secretary",
		scope: "user_bot",
	},
	{
		tab: "webhooks",
		label: "Webhook",
		desc: "外部サービスからの通知の受け口",
		category: "automation",
		only: "secretary",
		scope: "bot",
	},
	{
		tab: "discord",
		label: "Discord",
		desc: "Bot トークン・招待リンク（汎用属性は応答する場所と相手も）",
		category: "discord",
		scope: "bot",
		visible: (ctx) => !(ctx.systemDefaultBot && !ctx.isAdmin),
	},
];

/** 設定ハブ配下の子タブ集合（パンくず・アクティブ判定用。"settings" 自身は含まない）。 */
export const SETTINGS_CHILD_TABS: BotTab[] = SETTINGS_HUB_ITEMS.map(
	(i) => i.tab,
);

/** 設定ハブ配下のタブの行定義（設定ハブ外のタブは undefined）。 */
export function hubItemOf(tab: BotTab): SettingsHubItem | undefined {
	return SETTINGS_HUB_ITEMS.find((i) => i.tab === tab);
}

export interface HubGroup {
	category: SettingsCategory;
	items: SettingsHubItem[];
	/** プリセットのせいで行が無い（items は空で、unavailableNote を出す）。 */
	unavailable: boolean;
}

/**
 * ハブに出すカテゴリと行を組む。行が無いカテゴリは、プリセットだけが理由なら案内付きで残し、
 * それ以外（権限・既定 Bot）なら出さない。
 */
export function hubGroups(ctx: HubContext): HubGroup[] {
	const groups: HubGroup[] = [];
	for (const category of SETTINGS_CATEGORIES) {
		const inCategory = SETTINGS_HUB_ITEMS.filter(
			(i) => i.category === category.id,
		);
		const allowed = inCategory.filter((i) => !i.visible || i.visible(ctx));
		const items = allowed.filter((i) => !i.only || i.only === ctx.preset);
		if (items.length > 0) {
			groups.push({ category, items, unavailable: false });
		} else if (allowed.length > 0 && category.unavailableNote) {
			groups.push({ category, items: [], unavailable: true });
		}
	}
	return groups;
}
