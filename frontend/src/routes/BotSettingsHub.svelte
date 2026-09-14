<script lang="ts">
	// ─────────────────────────────────────────────────────────────────────────
	// BotSettingsHub — 設定ハブ（/bot/settings）。
	//
	// サイドバー2階層化に伴い、低頻度の設定系タブ（ペルソナ/Playbook/MCP/
// 配信/Webhook/Discord/Bot基本設定/接続端末）の入口を縦リストに集約する。
// 各行は既存の /bot/<tab> へ遷移するだけで、遷移先ページ自体は不変。
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

	<div class="settings-hub-list">
		{#each items as item (item.tab)}
			<button
				type="button"
				class="settings-hub-row"
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
	.settings-hub-list {
		border-top: 1px solid var(--border-matte);
		margin-top: 16px;
	}
	.settings-hub-row {
		display: flex;
		align-items: center;
		gap: 14px;
		width: 100%;
		min-height: 76px;
		padding: 14px 16px;
		text-align: left;
		cursor: pointer;
		font: inherit;
		color: var(--text-primary);
		background: transparent;
		border: 0;
		border-bottom: 1px solid var(--border-matte);
		transition: background-color 0.15s ease;
	}
	.settings-hub-row:hover,
	.settings-hub-row:focus-visible {
		background: var(--surface-1dp);
		outline: none;
	}
	.settings-hub-row :global(.settings-hub-icon) {
		flex-shrink: 0;
		color: var(--color-primary);
	}
	.settings-hub-row :global(.settings-hub-chevron) {
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
	@media (max-width: 600px) { .settings-hub-row { padding-inline: 12px; } }
</style>
