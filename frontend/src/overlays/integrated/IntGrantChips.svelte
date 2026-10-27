<script lang="ts">
// リソース許可 UI（旧 intGrantChips + wireIntGrantChips を {#each} + onclick/onchange 直結へ）。
// 許可済み Bot は「× で解除できるチップ」、未許可 Bot は ＋ から開く一覧で選んで追加する。
// 手動 innerHTML/intEsc は不要（Svelte 自動エスケープ）。トグルは親の ontoggle に委譲。
import type { IntegratedBotView } from "$lib/api/types";
import { Select } from "$lib/components/ui";

interface Props {
	/** 全 Bot 一覧（overview.bots） */
	bots: IntegratedBotView[];
	/** 許可済み botId の集合 */
	grantedIds: Set<string>;
	/** (botId, granted) を受け取るトグル。granted=true で付与, false で解除 */
	ontoggle: (botId: string, granted: boolean) => void;
}

let { bots, grantedIds, ontoggle }: Props = $props();

const granted = $derived(bots.filter((b) => grantedIds.has(b.id)));
const available = $derived(bots.filter((b) => !grantedIds.has(b.id)));
const availableOptions = $derived(
	available.map((b) => ({ value: b.id, label: b.name })),
);
// 選んだら許可して、＋ の表示に戻す
let picked = $state("");

function onSelect(botId: string) {
	picked = "";
	if (botId) ontoggle(botId, true);
}
</script>

<div class="int-grant-chips">
	{#if granted.length}
		{#each granted as b (b.id)}
			<span class="int-chip">
				{b.name}
				<button
					type="button"
					class="int-chip-remove"
					title="許可を解除"
					aria-label="{b.name} の許可を解除"
					onclick={() => ontoggle(b.id, false)}>×</button
				>
			</span>
		{/each}
	{:else}
		<span class="int-chip-empty">許可中のBotはありません</span>
	{/if}

	{#if available.length}
		<Select
			class="int-chip-add"
			title="Botを追加"
			ariaLabel="許可するBotを追加"
			options={availableOptions}
			bind:value={picked}
			onchange={onSelect}
		>
			{#snippet trigger()}<span aria-hidden="true">＋</span>{/snippet}
		</Select>
	{/if}
</div>

<style>
	.int-grant-chips {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px;
		margin-top: 8px;
	}
	.int-chip {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-size: 0.78rem;
		background: var(--surface-1dp, rgba(255, 255, 255, 0.04));
		border: 1px solid var(--border-matte, #333);
		border-radius: 6px;
		padding: 3px 6px 3px 10px;
	}
	.int-chip-remove {
		background: none;
		border: 0;
		color: var(--text-secondary, #a1a1aa);
		cursor: pointer;
		font-size: 1rem;
		line-height: 1;
		padding: 0 2px;
	}
	.int-chip-empty {
		font-size: 0.75rem;
		color: var(--text-secondary, #71717a);
	}
	/* 入力欄の見た目ではなく、チップの横に置く小さな ＋ ボタンにする */
	.int-grant-chips :global(.int-chip-add),
	.int-grant-chips :global(.int-chip-add:focus-within) {
		border: 0;
		background: transparent;
	}
	.int-grant-chips :global(.int-chip-add .trigger) {
		padding: 2px 6px;
		font-size: 1rem;
		line-height: 1;
		color: var(--text-secondary, #a1a1aa);
	}
</style>
