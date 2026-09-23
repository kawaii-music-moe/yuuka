<script lang="ts">
// 収支を手動で記録するフォーム（旧 index.html #expense-form + app.js:4151 expenseForm submit）。
// 保存は onsave で親へ委譲。time は送信時に現在時刻を付与。

import type { ExpenseType } from "$lib/api/types";
import { Button } from "$lib/components/ui";
import { EXPENSE_CATEGORIES, nowTime, todayIso } from "./expenseUtils";

interface Props {
	onsave: (payload: {
		type: ExpenseType;
		amount: number;
		category: string;
		description: string;
		date: string;
		time: string;
	}) => void;
}

let { onsave }: Props = $props();

let type = $state<ExpenseType>("expense");
let amount = $state<number | null>(null);
let category = $state<string>(EXPENSE_CATEGORIES[0]);
let description = $state("");
let date = $state(todayIso());

function submit(e: SubmitEvent) {
	e.preventDefault();
	if (amount == null || Number.isNaN(amount)) return;
	onsave({
		type,
		amount: Number(amount),
		category,
		description: description.trim(),
		date,
		time: nowTime(),
	});
}

/** 親が登録成功後に呼ぶリセット（旧 expenseForm.reset() + 日付再設定）。 */
export function reset() {
	type = "expense";
	amount = null;
	category = EXPENSE_CATEGORIES[0];
	description = "";
	date = todayIso();
}
</script>

<div class="action-column card">
	<div class="column-header">
		<h3>
			<span class="material-symbols-outlined header-icon-symbol" aria-hidden="true"
				>edit_document</span
			>収支を手動で記録
		</h3>
	</div>
	<form onsubmit={submit}>
		<div class="form-row">
			<div class="form-group">
				<label for="exp-type">種別 *</label>
				<select id="exp-type" required bind:value={type}>
					<option value="expense">支出として記録</option>
					<option value="income">収入として記録</option>
				</select>
			</div>
			<div class="form-group">
				<label for="exp-amount">金額 (円) *</label>
				<input
					type="number"
					id="exp-amount"
					required
					placeholder="1200"
					bind:value={amount}
				/>
			</div>
			<div class="form-group">
				<label for="exp-category">カテゴリー *</label>
				<select id="exp-category" required bind:value={category}>
					{#each EXPENSE_CATEGORIES as cat (cat)}
						<option value={cat}>{cat}</option>
					{/each}
				</select>
			</div>
		</div>
		<div class="form-group">
			<label for="exp-description">メモ・用途</label>
			<input
				type="text"
				id="exp-description"
				placeholder="昼食代、書籍代など"
				bind:value={description}
			/>
		</div>
		<div class="form-group">
			<label for="exp-date">日付</label>
			<input type="date" id="exp-date" bind:value={date} />
		</div>
		<Button type="submit" variant="primary" block>経費登録</Button>
	</form>
</div>
