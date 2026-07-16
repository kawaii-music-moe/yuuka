// ─────────────────────────────────────────────────────────────────────────────
// botTabs — 設定ハブ（/bot/settings）構成の単一情報源（2階層化）。
// 「どのタブが設定ハブ配下か」とカード表示定義をここに一元管理し、
// BotShell（パンくず/サイドバーのアクティブ判定）と BotSettingsHub（カード一覧）
// の両方がここを参照する（2重リスト化による無言の乖離を防ぐ）。
// ─────────────────────────────────────────────────────────────────────────────
import type { BotTab } from "$lib/router";

/** Bot プリセット識別子（未知値・未設定は secretary 扱い）。 */
export type BotPreset = "secretary" | "mcp_assistant";

/** activeBot からプリセットを正規化（既定 secretary）。 */
export function botPreset(
	bot: { preset?: string } | null | undefined,
): BotPreset {
	return bot?.preset === "mcp_assistant" ? "mcp_assistant" : "secretary";
}

export interface SettingsHubItem {
	tab: BotTab;
	label: string;
	icon: string;
	desc: string;
	/** 表示プリセット限定（未指定は両方）。 */
	only?: BotPreset;
}

/** 設定ハブのカード定義（表示順）。 */
export const SETTINGS_HUB_ITEMS: SettingsHubItem[] = [
	{
		tab: "config",
		label: "Bot 基本設定",
		icon: "smart_toy",
		desc: "トークン・モデル・基本動作などのシステム設定",
	},
	{
		tab: "personas",
		label: "ペルソナ",
		icon: "theater_comedy",
		desc: "口調・性格など応答スタイルの管理",
	},
	{
		tab: "playbooks",
		label: "Playbook 管理",
		icon: "description",
		desc: "自動化手順書と定期実行スケジュール",
		only: "secretary",
	},
	{
		tab: "mcp",
		label: "MCPサーバー",
		icon: "extension",
		desc: "MCP サーバーの接続とツール利用設定",
	},
	{
		tab: "delivery",
		label: "配信設定",
		icon: "campaign",
		desc: "活動サマリーなどの定期配信設定",
		only: "secretary",
	},
	{
		tab: "webhooks",
		label: "Webhook",
		icon: "webhook",
		desc: "外部サービスからの Webhook 連携",
		only: "secretary",
	},
	{
		tab: "discord",
		label: "Discord連携",
		icon: "forum",
		desc: "Discord Bot の連携設定",
		only: "mcp_assistant",
	},
	{
		tab: "devices",
		label: "接続端末",
		icon: "devices",
		desc: "ログイン中の接続端末の確認・管理",
	},
];

/** 設定ハブ配下の子タブ集合（パンくず・アクティブ判定用。"settings" 自身は含まない）。 */
export const SETTINGS_CHILD_TABS: BotTab[] = SETTINGS_HUB_ITEMS.map(
	(i) => i.tab,
);

/** プリセットで表示対象カードを絞り込む。 */
export function filterHubItems(preset: BotPreset): SettingsHubItem[] {
	return SETTINGS_HUB_ITEMS.filter((i) => !i.only || i.only === preset);
}
