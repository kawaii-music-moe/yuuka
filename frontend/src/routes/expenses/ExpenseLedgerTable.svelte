<script lang="ts">
// 直近の支出履歴テーブル（旧 app.js:3859 fetchExpensesList の Ledger 描画部）。
import type { ExpenseRecord } from "$lib/api/types";
import { expenseSource, formatExpenseDate, yen } from "./expenseUtils";

interface Props {
	expenses: ExpenseRecord[];
}

let { expenses }: Props = $props();
</script>

<div class="table-responsive">
	<table class="expense-table">
		<thead>
			<tr>
				<th>日付</th>
				<th>カテゴリー</th>
				<th>メモ/詳細</th>
				<th>登録元</th>
				<th>金額</th>
			</tr>
		</thead>
		<tbody>
			{#if expenses.length === 0}
				<tr>
					<td colspan="5" class="ledger-empty-cell">家計簿データが空です。</td>
				</tr>
			{:else}
				{#each expenses as exp (exp.id)}
					{@const src = expenseSource(exp)}
					<tr>
						<td>{formatExpenseDate(exp)}</td>
						<td>{exp.category}</td>
						<td>{exp.memo || "説明なし"}</td>
						<td class="ledger-source-cell">
							<span class="material-symbols-outlined source-icon" aria-hidden="true"
								>{src.icon}</span
							>{src.label}
						</td>
						<td class="amount-cell" class:ledger-income={exp.type === "income"}>
							{exp.type === "income" ? `+${yen(exp.amount)}` : yen(exp.amount)}
						</td>
					</tr>
				{/each}
			{/if}
		</tbody>
	</table>
</div>

<style>
	.ledger-empty-cell {
		text-align: center;
	}
	.ledger-source-cell {
		font-size: 0.75rem;
		font-family: var(--font-family-mono);
	}
	.ledger-income {
		color: var(--color-green);
	}
</style>
