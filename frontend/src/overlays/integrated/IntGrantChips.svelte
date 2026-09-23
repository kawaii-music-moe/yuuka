<script lang="ts">
// リソース許可 UI（旧 intGrantChips + wireIntGrantChips を {#each} + onclick/onchange 直結へ）。
// 許可済み Bot は「× で解除できるチップ」、未許可 Bot は <select> から選んで追加する。
// 手動 innerHTML/intEsc は不要（Svelte 自動エスケープ）。トグルは親の ontoggle に委譲。
import type { IntegratedBotView } from "$lib/api/types";
import Icon from "$lib/components/ui/Icon.svelte";

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

function botName(b: IntegratedBotView): string {
	return b.is_system_default ? "既定の秘書（早瀬ユウカ）" : b.name;
}

function onSelect(e: Event) {
	const sel = e.currentTarget as HTMLSelectElement;
	const botId = sel.value;
	if (!botId) return;
	ontoggle(botId, true);
	sel.value = "";
}
</script>

<div class="int-grant-chips">
	{#if granted.length}
		{#each granted as b (b.id)}
			<span class="int-chip">
				{botName(b)}
				<button
					type="button"
					class="int-chip-remove"
					title="許可を解除"
					aria-label="{botName(b)} の許可を解除"
					onclick={() => ontoggle(b.id, false)}>×</button
				>
			</span>
		{/each}
	{:else}
		<span class="int-chip-empty">許可中のBotはありません</span>
	{/if}

	{#if available.length}
		<select
			class="int-chip-add"
			title="Botを追加"
			aria-label="許可するBotを追加"
			onchange={onSelect}
		>
			<option value="">＋</option>
			{#each available as b (b.id)}
				<option value={b.id}>{botName(b)}</option>
			{/each}
		</select>
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
	.int-chip-add {
		font-size: 1rem;
		line-height: 1;
		width: auto;
		padding: 2px 6px;
		border: 0;
		background: transparent;
		color: var(--text-secondary, #a1a1aa);
		cursor: pointer;
		appearance: none;
	}
</style>
