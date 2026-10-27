<script lang="ts">
// 収支を手動で記録するフォーム（旧 index.html #expense-form + app.js:4151 expenseForm submit）。
// 保存は onsave で親へ委譲。time は送信時に現在時刻を付与。

import type { ExpenseType } from "$lib/api/types";
import { Button } from "$lib/components/ui";
import InlineSelect from "../delivery/InlineSelect.svelte";
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

const TYPE_OPTIONS: { value: ExpenseType; label: string }[] = [
	{ value: "expense", label: "支出" },
	{ value: "income", label: "収入" },
];
const CATEGORY_OPTIONS = EXPENSE_CATEGORIES.map((c) => ({
	value: c,
	label: c,
}));

let type = $state<ExpenseType>("expense");
let amount = $state<number | null>(null);
let category = $state<string>(EXPENSE_CATEGORIES[0]);
let description = $state("");
let date = $state(todayIso());

// 日付は欄のどこを押してもブラウザ標準の日付ピッカーを開く（未対応ブラウザでは何もしない）。
function openPicker(e: MouseEvent) {
	const input = e.currentTarget as HTMLInputElement;
	try {
		input.showPicker?.();
	} catch {
		// ユーザー操作扱いにならない等で開けないときは、通常の入力欄のままにする
	}
}

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
				<label for="exp-amount">種別・金額 (円) *</label>
				<div class="joined-field framed-field amount-field">
					<InlineSelect
						id="exp-type"
						ariaLabel="種別"
						options={TYPE_OPTIONS}
						bind:value={type}
					/>
					<span class="divider" aria-hidden="true"></span>
					<input
						type="number"
						id="exp-amount"
						required
						placeholder="1200"
						bind:value={amount}
					/>
				</div>
			</div>
			<div class="form-group">
				<label for="exp-category">カテゴリー *</label>
				<div class="joined-field framed-field">
					<InlineSelect
						id="exp-category"
						class="category-select"
						options={CATEGORY_OPTIONS}
						bind:value={category}
					/>
				</div>
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
			<input
				type="date"
				id="exp-date"
				class="picker-input"
				onclick={openPicker}
				bind:value={date}
			/>
		</div>
		<Button type="submit" variant="primary" block>経費登録</Button>
	</form>
</div>

<style>
	/* このページの他の入力欄と同じく、枠全体に枠線とフォーカスリングを付ける。 */
	.framed-field {
		border: 1px solid var(--border-divider);
		transition:
			border-color 0.15s ease,
			box-shadow 0.15s ease;
	}
	.framed-field:focus-within {
		border-color: var(--color-primary);
		box-shadow: 0 0 0 1px var(--color-primary);
	}
	/* カテゴリーは単独のプルダウンなので枠いっぱいに広げる */
	.framed-field :global(.category-select) {
		flex: 1;
	}
	.amount-field input,
	.amount-field input:focus {
		flex: 1;
		width: auto;
		min-width: 0;
		border: 0;
		box-shadow: none;
		background: transparent;
		/* ブラウザ標準の増減ボタン（▲▼）を出さない */
		appearance: textfield;
	}
	.amount-field input::-webkit-outer-spin-button,
	.amount-field input::-webkit-inner-spin-button {
		appearance: none;
		margin: 0;
	}
	.picker-input {
		cursor: pointer;
	}
</style>
