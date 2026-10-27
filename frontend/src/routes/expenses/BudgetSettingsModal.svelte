<script lang="ts">
// カテゴリ別予算上限設定モーダル（旧 index.html #modal-budget-settings
// + app.js:4267 budgetLimitForm submit）。
// 設定済みの上限は表示しない（進捗は HUD のカテゴリー別バーで見る）。
// 保存の API は親が担当（子は API を叩かない）。追加は onadd で親へ委譲する。

import { Button, Modal, Select } from "$lib/components/ui";
import { pushToast } from "$lib/stores/toast";
import { EXPENSE_CATEGORIES, EXPENSE_CATEGORY_OPTIONS } from "./expenseUtils";

interface Props {
	open?: boolean;
	onadd: (payload: { category: string; limitAmount: number }) => void;
}

let { open = $bindable(false), onadd }: Props = $props();

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
	<form class="budget-settings-form" onsubmit={submit}>
		<div class="form-row">
			<div class="form-group">
				<label for="budget-category">カテゴリー</label>
				<Select
					id="budget-category"
					options={EXPENSE_CATEGORY_OPTIONS}
					bind:value={category}
				/>
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
	.budget-settings-form {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
</style>
