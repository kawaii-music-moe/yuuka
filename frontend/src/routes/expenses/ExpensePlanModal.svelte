<script lang="ts">
// 支払い予定の追加モーダル（旧 index.html #modal-expense-plan + app.js expensePlanForm submit）。
// 保存は onsave で親へ委譲。開くたびに初期化（日付は今日）。
import { Button, Modal } from "$lib/components/ui";
import { EXPENSE_CATEGORIES, todayIso } from "./expenseUtils";

interface Props {
	open?: boolean;
	onsave: (payload: {
		title: string;
		amount: number;
		category: string;
		plannedDate: string;
		description: string;
	}) => void;
}

let { open = $bindable(false), onsave }: Props = $props();

let title = $state("");
let amount = $state<number | null>(null);
let category = $state<string>(EXPENSE_CATEGORIES[0]);
let plannedDate = $state("");
let description = $state("");

$effect(() => {
	if (!open) return;
	title = "";
	amount = null;
	category = EXPENSE_CATEGORIES[0];
	plannedDate = todayIso();
	description = "";
});

// 支払予定日は欄のどこを押してもブラウザ標準の日付ピッカーを開く（未対応ブラウザでは何もしない）。
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
	const trimmed = title.trim();
	if (!trimmed || amount == null || Number.isNaN(amount) || !plannedDate)
		return;
	onsave({
		title: trimmed,
		amount: Number(amount),
		category,
		plannedDate,
		description: description.trim(),
	});
}
</script>

<Modal bind:open title="支払い予定の追加">
	<form onsubmit={submit}>
		<div class="form-group">
			<label for="plan-title">タイトル *</label>
			<input
				type="text"
				id="plan-title"
				required
				placeholder="例: 電気代・家賃"
				bind:value={title}
			/>
		</div>
		<div class="form-row">
			<div class="form-group">
				<label for="plan-amount">金額 (円) *</label>
				<input
					type="number"
					id="plan-amount"
					class="no-spin"
					required
					placeholder="5000"
					bind:value={amount}
				/>
			</div>
			<div class="form-group">
				<label for="plan-category">カテゴリー *</label>
				<select id="plan-category" required bind:value={category}>
					{#each EXPENSE_CATEGORIES as cat (cat)}
						<option value={cat}>{cat}</option>
					{/each}
				</select>
			</div>
		</div>
		<div class="form-group">
			<label for="plan-date">支払予定日 *</label>
			<input
				type="date"
				id="plan-date"
				class="picker-input"
				required
				onclick={openPicker}
				bind:value={plannedDate}
			/>
		</div>
		<div class="form-group">
			<label for="plan-description">メモ</label>
			<input
				type="text"
				id="plan-description"
				placeholder="備考など"
				bind:value={description}
			/>
		</div>
		<Button type="submit" variant="primary" block>予定を登録</Button>
	</form>
</Modal>

<style>
	/* ブラウザ標準の増減ボタン（▲▼）を出さない */
	.no-spin {
		appearance: textfield;
	}
	.no-spin::-webkit-outer-spin-button,
	.no-spin::-webkit-inner-spin-button {
		appearance: none;
		margin: 0;
	}
	.picker-input {
		cursor: pointer;
	}
</style>
