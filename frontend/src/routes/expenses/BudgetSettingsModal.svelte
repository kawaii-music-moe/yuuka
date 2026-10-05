<script lang="ts">
// カテゴリ別予算上限設定モーダル（旧 index.html #modal-budget-settings
// + app.js:4267 renderBudgetSettingsList / budgetLimitForm submit）。
// 一覧・追加・削除の API は親が担当（子は API を叩かない）。limits は親から受け取り、
// 追加は onadd、削除は ondelete で親へ委譲する。

import type { BudgetLimit } from "$lib/api/types";
import { Button, Modal } from "$lib/components/ui";
import { pushToast } from "$lib/stores/toast";
import { EXPENSE_CATEGORIES, yen } from "./expenseUtils";

interface Props {
	open?: boolean;
	limits: BudgetLimit[];
	onadd: (payload: { category: string; limitAmount: number }) => void;
	ondelete: (category: string) => void;
}

let { open = $bindable(false), limits, onadd, ondelete }: Props = $props();

let category = $state<string>(EXPENSE_CATEGORIES[0]);
let limitAmount = $state<number | null>(null);

$effect(() => {
	if (!open) return;
	category = EXPENSE_CATEGORIES[0];
	limitAmount = null;
});

function submit(e: SubmitEvent) {
	e.preventDefault();
	// 旧: category と有効な上限金額（>0）を検証。
	if (
		!category ||
		limitAmount == null ||
		Number.isNaN(limitAmount) ||
		limitAmount <= 0
	) {
		pushToast("カテゴリーと有効な上限金額を入力してください。", "error");
		return;
	}
	onadd({ category, limitAmount: Number(limitAmount) });
	limitAmount = null;
}
</script>

<Modal bind:open title="カテゴリ別予算上限設定">
	<p class="budget-settings-desc">
		カテゴリごとに月間の予算上限金額を設定します。未設定のカテゴリは上限なし（バー非表示）になります。
	</p>

	<div class="budget-settings-list">
		{#if limits.length === 0}
			<p class="budget-settings-empty">設定済みの上限はありません。</p>
		{:else}
			{#each limits as lim (lim.category)}
				<div class="budget-settings-row">
					<span class="budget-settings-cat">{lim.category}</span>
					<div class="budget-settings-right">
						<span class="budget-settings-amount">{yen(lim.limit_amount)}</span>
						<button
							type="button"
							class="btn btn-secondary btn-sm budget-del-btn"
							onclick={() => ondelete(lim.category)}>削除</button
						>
					</div>
				</div>
			{/each}
		{/if}
	</div>

	<form class="budget-settings-form" onsubmit={submit}>
		<div class="form-row">
			<div class="form-group">
				<label for="budget-category">カテゴリー</label>
				<select id="budget-category" bind:value={category}>
					{#each EXPENSE_CATEGORIES as cat (cat)}
						<option value={cat}>{cat}</option>
					{/each}
				</select>
			</div>
			<div class="form-group">
				<label for="budget-limit-amount">上限金額 (円)</label>
				<input
					type="number"
					id="budget-limit-amount"
					placeholder="50000"
					min="1"
					bind:value={limitAmount}
				/>
			</div>
		</div>
		<Button type="submit" variant="primary" block>設定を保存</Button>
	</form>
</Modal>

<style>
	.budget-settings-desc {
		margin-bottom: 16px;
	}
	.budget-settings-list {
		display: flex;
		flex-direction: column;
		gap: 4px;
		margin-bottom: 16px;
		max-height: 180px;
		overflow-y: auto;
		padding-right: 6px;
	}
	.budget-settings-empty {
		font-size: 0.8rem;
		color: var(--text-secondary);
		padding: 8px 0;
	}
	.budget-settings-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 10px 0;
		border-bottom: 1px solid var(--border-divider);
	}
	.budget-settings-cat {
		font-size: 0.9rem;
		font-weight: 500;
		color: var(--text-primary);
	}
	.budget-settings-right {
		display: flex;
		align-items: center;
		gap: 12px;
	}
	.budget-settings-amount {
		font-size: 0.9rem;
		font-weight: 700;
		color: var(--text-primary);
	}
	.budget-del-btn {
		font-size: 0.72rem;
		padding: 4px 10px;
		text-transform: none;
		letter-spacing: normal;
		font-weight: normal;
		height: auto;
	}
	.budget-settings-form {
		display: flex;
		flex-direction: column;
		gap: 12px;
		margin-top: 8px;
		border-top: 1px solid var(--border-divider);
		padding-top: 16px;
	}
</style>
