<script lang="ts">
// 支払い予定テーブル（旧 app.js:4024 renderExpensePlans）。
// pending 行のみ「支払う」「キャンセル」ボタン。期日超過は赤ハイライト。

import type { PlannedPayment } from "$lib/api/types";
import { Icon } from "$lib/components/ui";
import { todayIso, yen } from "./expenseUtils";

interface Props {
	plans: PlannedPayment[];
	onPay: (plan: PlannedPayment) => void;
	onCancel: (plan: PlannedPayment) => void;
}

let { plans, onPay, onCancel }: Props = $props();

const today = todayIso();
</script>

<div class="table-responsive">
	<table class="expense-table">
		<thead>
			<tr>
				<th>予定日</th>
				<th>タイトル</th>
				<th>カテゴリー</th>
				<th>メモ</th>
				<th>金額</th>
				<th class="plans-actions-th">操作</th>
			</tr>
		</thead>
		<tbody>
			{#if plans.length === 0}
				<tr>
					<td colspan="6" class="plans-empty-cell">支払い予定はありません。</td>
				</tr>
			{:else}
				{#each plans as plan (plan.id)}
					{@const isPending = plan.status === "pending"}
					{@const isOverdue = isPending && plan.due_date <= today}
					<tr class:plan-overdue-row={isOverdue}>
						<td class:plan-overdue-date={isOverdue}>
							{plan.due_date}
							{#if plan.repeat_rule}
								<span
									class="tag-chip plan-repeat-badge"
									title={`繰り返し: ${plan.repeat_rule}`}>🔁</span
								>
							{/if}
						</td>
						<td>{plan.title}</td>
						<td>{plan.category}</td>
						<td class="plan-memo-cell">{plan.memo || "—"}</td>
						<td class="amount-cell">{yen(plan.amount)}</td>
						<td class="plan-actions-cell">
							{#if isPending}
								<button
									type="button"
									class="btn btn-primary btn-sm plan-action-btn"
									onclick={() => onPay(plan)}>支払う</button
								>
								<button
									type="button"
									class="btn btn-secondary btn-sm plan-action-btn plan-cancel-btn"
									title="支払い予定をキャンセル"
									aria-label="支払い予定をキャンセル"
									onclick={() => onCancel(plan)}
								>
									<Icon name="delete" size={15} />
								</button>
							{:else}
								<span
									class="status-badge {plan.status === 'settled'
										? 'status-sent'
										: 'status-cancelled'}"
								>
									{plan.status === "settled" ? "消込済み" : "キャンセル"}
								</span>
							{/if}
						</td>
					</tr>
				{/each}
			{/if}
		</tbody>
	</table>
</div>

<style>
	.plans-actions-th {
		text-align: right;
		width: 100px;
	}
	.plans-empty-cell {
		text-align: center;
	}
	.plan-overdue-row {
		background: rgba(207, 102, 121, 0.07);
	}
	.plan-overdue-date {
		color: var(--color-red);
	}
	.plan-repeat-badge {
		margin-left: 6px;
	}
	.plan-memo-cell {
		color: var(--text-secondary);
	}
	.plan-actions-cell {
		display: flex;
		justify-content: flex-end;
		align-items: center;
		gap: 4px;
	}
	.plan-action-btn {
		font-size: 0.72rem;
		padding: 3px 8px;
		line-height: 1;
	}
	.plan-cancel-btn {
		display: inline-flex;
		align-items: center;
	}
</style>
