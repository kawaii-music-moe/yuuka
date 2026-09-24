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
			<input type="date" id="plan-date" required bind:value={plannedDate} />
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
