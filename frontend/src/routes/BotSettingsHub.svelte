<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// BotSettingsHub — 設定ハブ（/bot/settings）。
//
// サイドバー2階層化に伴い、低頻度の設定系タブ（ペルソナ/Playbook/MCP/
// 配信/Webhook/Discord/Bot基本設定/接続端末）の入口を縦並びリスト（罫線区切り）に集約する。
// 各行は既存の /bot/<tab> へ遷移するだけで、遷移先ページ自体は不変。
// 項目定義・プリセット別表示条件は $lib/botTabs が単一情報源
// （BotShell のパンくず/アクティブ判定も同じ集合を参照する）。
// ─────────────────────────────────────────────────────────────────────────

import { botPreset, filterHubItems, type SettingsHubItem } from "$lib/botTabs";
import { navigateTo } from "$lib/router";
import { activeBot } from "$lib/stores/activeBot";

const items = $derived(filterHubItems(botPreset($activeBot)));
type HubGroup = { category?: string; items: SettingsHubItem[] };
const groups = $derived.by(() => {
	const result: HubGroup[] = [];
	const byCategory = new Map<string | undefined, HubGroup>();
	for (const item of items) {
		let group = byCategory.get(item.category);
		if (!group) {
			group = { category: item.category, items: [] };
			byCategory.set(item.category, group);
			result.push(group);
		}
		group.items.push(item);
	}
	return result;
});
</script>

<section class="tab-view">
	<div class="settings-hub-list">
		{#each groups as group, groupIndex (`${group.category ?? "uncategorized"}-${groupIndex}`)}
			{#if group.category}
				<h3 class="settings-hub-category">{group.category}</h3>
			{/if}
			{#each group.items as item (item.tab)}
				<button
					type="button"
					class="settings-hub-item"
					onclick={() => navigateTo(`/bot/${item.tab}`)}
				>
					<span class="settings-hub-text">
						<span class="settings-hub-label">{item.label}</span>
						<span class="settings-hub-desc">{item.desc}</span>
					</span>
				</button>
			{/each}
		{/each}
	</div>
</section>

<style>
	/* 縦並びリスト。カード枠は持たず、行間を罫線で区切る */
	.settings-hub-list {
		display: flex;
		flex-direction: column;
		margin-top: 16px;
		border-top: 1px solid var(--border-divider);
	}
	.settings-hub-item {
		display: flex;
		align-items: center;
		gap: 14px;
		width: 100%;
		padding: 16px 12px;
		text-align: left;
		cursor: pointer;
		font: inherit;
		color: var(--text-primary);
		background: none;
		border: none;
		border-bottom: 1px solid var(--border-divider);
		transition: background-color 0.15s ease;
	}
	.settings-hub-item:hover,
	.settings-hub-item:focus-visible {
		background-color: var(--surface-1dp);
	}
	.settings-hub-category {
		margin: 18px 12px 4px;
		font-size: 0.9rem;
		font-weight: 700;
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
</style>
