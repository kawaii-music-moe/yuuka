<script lang="ts">
	// ─────────────────────────────────────────────────────────────────────────
	// BotSettingsHub — 設定ハブ（/bot/settings）。
	//
	// サイドバー2階層化に伴い、低頻度の設定系タブ（ペルソナ/Playbook/MCP/
	// 配信/Webhook/Discord/Bot基本設定/接続端末）の入口をカード一覧に集約する。
	// 各カードは既存の /bot/<tab> へ遷移するだけで、遷移先ページ自体は不変。
	// カード定義・プリセット別表示条件は $lib/botTabs が単一情報源
	// （BotShell のパンくず/アクティブ判定も同じ集合を参照する）。
	// ─────────────────────────────────────────────────────────────────────────
	import { navigateTo } from "$lib/router";
	import { activeBot } from "$lib/stores/activeBot";
	import { botPreset, filterHubItems } from "$lib/botTabs";
	import { Icon } from "$lib/components/ui";

	const items = $derived(filterHubItems(botPreset($activeBot)));
</script>

<section class="tab-view">
	<p class="description-text">
		Bot の動作・連携・システムに関する設定の入口です。タスクや予定などの日常のデータ管理はサイドバーから直接開けます。
	</p>

	<div class="settings-hub-grid">
		{#each items as item (item.tab)}
			<button
				type="button"
				class="hover-lift settings-hub-card"
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
