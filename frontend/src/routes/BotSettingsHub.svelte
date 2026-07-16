<script lang="ts">
	// ─────────────────────────────────────────────────────────────────────────
	// BotSettingsHub — 設定ハブ（/bot/settings）。
	//
	// サイドバー2階層化に伴い、低頻度の設定系タブ（ペルソナ/Playbook/MCP/
	// 配信/Webhook/Discord/Bot基本設定/接続端末）の入口をカード一覧に集約する。
	// 各カードは既存の /bot/<tab> へ遷移するだけで、遷移先ページ自体は不変。
	// プリセット別の表示条件は旧サイドバーの SECRETARY/ASSISTANT_ONLY と同一。
	// ─────────────────────────────────────────────────────────────────────────
	import { derived } from "svelte/store";
	import { navigateTo, type BotTab } from "$lib/router";
	import { activeBot } from "$lib/stores/activeBot";
	import { Icon } from "$lib/components/ui";

	interface HubItem {
		tab: BotTab;
		label: string;
		icon: string;
		desc: string;
		/** 表示プリセット限定（未指定は両方）。 */
		only?: "secretary" | "assistant";
	}

	const ALL_ITEMS: HubItem[] = [
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
			only: "assistant",
		},
		{
			tab: "devices",
			label: "接続端末",
			icon: "devices",
			desc: "ログイン中の接続端末の確認・管理",
		},
	];

	// プリセット別に絞り込んだカード配列（BotShell の menuItems と同じ規約）。
	const items = derived(activeBot, ($bot) => {
		const isAssistant = ($bot?.preset ?? "secretary") === "mcp_assistant";
		return ALL_ITEMS.filter((i) => {
			if (!i.only) return true;
			return i.only === (isAssistant ? "assistant" : "secretary");
		});
	});
</script>

<section class="tab-view">
	<p class="description-text">
		Bot の動作・連携・システムに関する設定の入口です。タスクや予定などの日常のデータ管理はサイドバーから直接開けます。
	</p>

	<div class="settings-hub-grid">
		{#each $items as item (item.tab)}
			<button
				type="button"
				class="glass hover-lift settings-hub-card"
				onclick={() => navigateTo(`/bot/${item.tab}`)}
			>
				<Icon name={item.icon} size={28} class="settings-hub-icon" />
				<span class="settings-hub-text">
					<span class="settings-hub-label">{item.label}</span>
					<span class="settings-hub-desc">{item.desc}</span>
				</span>
				<Icon name="chevron_right" class="settings-hub-chevron" />
			</button>
		{/each}
	</div>
</section>

<style>
	.settings-hub-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
		gap: 12px;
		margin-top: 16px;
	}
	.settings-hub-card {
		display: flex;
		align-items: center;
		gap: 14px;
		padding: 18px 16px;
		text-align: left;
		cursor: pointer;
		font: inherit;
		color: var(--text-primary);
		background-color: var(--surface-1dp);
		border: 1px solid var(--border-matte);
		border-radius: var(--radius);
	}
	.settings-hub-card :global(.settings-hub-icon) {
		flex-shrink: 0;
		color: var(--color-primary);
	}
	.settings-hub-card :global(.settings-hub-chevron) {
		flex-shrink: 0;
		margin-left: auto;
		color: var(--text-secondary);
	}
	.settings-hub-text {
		display: flex;
		flex-direction: column;
		gap: 4px;
		min-width: 0;
	}
	.settings-hub-label {
		font-weight: 600;
	}
	.settings-hub-desc {
		font-size: 0.82rem;
		color: var(--text-secondary);
	}
	@media (max-width: 600px) {
		.settings-hub-grid {
			grid-template-columns: 1fr;
		}
	}
</style>
