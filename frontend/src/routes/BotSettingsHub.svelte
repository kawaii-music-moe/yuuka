<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// BotSettingsHub — 設定ハブ（/bot/settings）。
//
// 設定系タブの入口をカテゴリ見出し付きの縦並びリスト（罫線区切り）にまとめる
// （構成は docs/design/settings-restructure.md「Bot設定」）。各行は /bot/<tab> へ遷移するだけ。
// カテゴリの並びはプリセットで変えず、プリセットで使えないカテゴリは 1 行の案内に畳む。
// 項目定義・表示条件は $lib/botTabs が単一情報源
// （BotShell のパンくず/アクティブ判定も同じ集合を参照する）。
// ─────────────────────────────────────────────────────────────────────────

import { botPreset, hubGroups } from "$lib/botTabs";
import { Icon } from "$lib/components/ui";
import { navigateTo } from "$lib/router";
import { activeBot } from "$lib/stores/activeBot";
import { isAdmin } from "$lib/stores/session";
import { isDefaultBot, isSystemDefaultBot } from "./config/configTypes";

const groups = $derived(
	hubGroups({
		preset: botPreset($activeBot),
		defaultBot: isDefaultBot($activeBot?.id),
		systemDefaultBot: isSystemDefaultBot($activeBot?.id),
		isAdmin: $isAdmin,
	}),
);
</script>

<section class="tab-view">
	<div class="settings-hub-list">
		{#each groups as group (group.category.id)}
			<h3 class="settings-hub-category">{group.category.label}</h3>
			{#if group.unavailable}
				<p class="settings-hub-unavailable">
					<Icon name="info" size={16} />{group.category.unavailableNote}
				</p>
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
					<Icon name="chevron_right" class="settings-hub-chevron" />
				</button>
			{/each}
		{/each}
	</div>
</section>

<style>
	/* 縦並びリスト。カード枠は持たず、行間を罫線で区切る（先頭は常にカテゴリ見出し） */
	.settings-hub-list {
		display: flex;
		flex-direction: column;
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
	.settings-hub-item :global(.settings-hub-chevron) {
		flex-shrink: 0;
		margin-left: auto;
		color: var(--text-secondary);
	}
	.settings-hub-category {
		margin: 18px 12px 4px;
		font-size: 0.9rem;
		font-weight: 700;
		color: var(--text-secondary);
	}
	/* プリセットで使えないカテゴリの畳み表示（行の代わりの 1 行・押せない） */
	.settings-hub-unavailable {
		display: flex;
		align-items: center;
		gap: 8px;
		margin: 0;
		padding: 14px 12px;
		font-size: 0.85rem;
		color: var(--text-secondary);
		border-bottom: 1px solid var(--border-divider);
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
