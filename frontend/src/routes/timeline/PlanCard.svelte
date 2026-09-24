<script lang="ts">
// 計画ブロックカード（旧 app.js buildPlanCard）。
import type { DayPlanBlock } from "$lib/api/types";
import { Icon } from "$lib/components/ui";
import { planTimeRange, planTypeIcon, transitText } from "./timelineUtils";

interface Props {
	block: DayPlanBlock;
	onedit: (block: DayPlanBlock) => void;
	ondelete: (id: number) => void;
}

let { block, onedit, ondelete }: Props = $props();

const timeStr = $derived(planTimeRange(block.start_time, block.end_time));
const hasTransit = $derived(!!(block.transit_from || block.transit_to));
</script>

<div class="tl-plan-card" data-type={block.type}>
	{#if timeStr}
		<div class="tl-plan-time">{timeStr}</div>
	{/if}

	<div class="tl-plan-main">
		<Icon name={planTypeIcon(block.type)} class="tl-plan-icon" />
		<div class="tl-plan-title">{block.title}</div>
	</div>

	{#if hasTransit}
		<div class="tl-plan-sub">
			{transitText(block.transit_from, block.transit_to, block.transit_line)}
		</div>
	{/if}
	{#if block.description}
		<div class="tl-plan-sub">{block.description}</div>
	{/if}

	<div class="tl-plan-actions">
		<button
			type="button"
			class="btn-icon-sm"
			aria-label="編集"
			onclick={() => onedit(block)}
		>
			<Icon name="edit" />
		</button>
		<button
			type="button"
			class="btn-icon-sm tl-del-btn"
			aria-label="削除"
			onclick={() => ondelete(block.id)}
		>
			<Icon name="delete" />
		</button>
	</div>
</div>

<style>
	.tl-plan-main {
		display: flex;
		align-items: center;
		gap: 6px;
	}
	.tl-plan-main :global(.tl-plan-icon) {
		font-size: 1rem;
		flex-shrink: 0;
	}
	.tl-del-btn {
		color: var(--color-red);
	}
</style>
