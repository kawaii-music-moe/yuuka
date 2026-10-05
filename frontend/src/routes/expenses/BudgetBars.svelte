<script lang="ts">
// カテゴリー別上限進捗バー（旧 app.js:3984 renderCategoryBudgetBars）。
// {@html} を使わず {#each} で <div> をバインド（§11.4）。
import type { BudgetLimit, CategoryTotal } from "$lib/api/types";
import { buildBudgetBars, yen } from "./expenseUtils";

interface Props {
	breakdown: CategoryTotal[];
	limits: BudgetLimit[];
}

let { breakdown, limits }: Props = $props();

const bars = $derived(buildBudgetBars(breakdown, limits));
</script>

{#if bars.length === 0}
	<p class="budget-empty">
		予算上限が設定されていません。「上限設定」ボタンから設定してください。
	</p>
{:else}
	{#each bars as bar (bar.category)}
		<div class="budget-bar-row">
			<div class="budget-bar-head">
				<span>{bar.category}</span>
				<span>
					{yen(bar.spent)} / {yen(bar.limit)} ({Math.round(bar.pct)}%)
				</span>
			</div>
			<div class="budget-bar-track">
				<div
					class="budget-bar-fill"
					style="width:{bar.pct}%;background:{bar.color};"
				></div>
			</div>
		</div>
	{/each}
{/if}

<style>
	.budget-empty {
		font-size: 0.8rem;
		color: var(--text-secondary);
	}
	.budget-bar-row {
		display: flex;
		flex-direction: column;
		gap: 3px;
	}
	.budget-bar-head {
		display: flex;
		justify-content: space-between;
		font-size: 0.78rem;
		color: var(--text-secondary);
	}
	.budget-bar-track {
		background: var(--surface-4dp);
		border-radius: 3px;
		height: 6px;
		overflow: hidden;
	}
	.budget-bar-fill {
		height: 100%;
		border-radius: 3px;
		transition: width 0.4s ease;
	}
</style>
