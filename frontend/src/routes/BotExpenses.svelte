<script lang="ts">
	// ─────────────────────────────────────────────────────────────────────────
	// 経費（家計簿）タブ。旧 app.js の fetchExpensesList / renderCategoryBudgetBars /
	// renderExpensePlans / renderBudgetSettingsList / handleReceiptScan・各フォーム submit
	// + index.html #tab-expenses（＋支払い予定・予算上限・レシート結果モーダル）を移植。
	//
	// 挙動の忠実移植:
	//   - HUD: 当月支出/収入合計・支払い予定件数（本日以前は赤）。
	//   - カテゴリー別上限進捗バー（BudgetBars）。「上限設定」で BudgetSettingsModal。
	//   - 支払い予定テーブル（pay=消込 / cancel）。「＋ 予定追加」で ExpensePlanModal。
	//   - AIレシートスキャナー（画像 base64 → financeApi.uploadReceipt → 結果モーダル）。
	//   - 手動記録フォーム（ExpenseForm）。
	//   - 直近30件の Ledger（ExpenseLedgerTable）。
	//   - activeBot 変更でリロード（bot-scoped API）。
	// ─────────────────────────────────────────────────────────────────────────
	import { activeBot } from "$lib/stores/activeBot";
	import { financeApi } from "$lib/api/services";
	import { ApiError } from "$lib/api/client";
	import { pushToast } from "$lib/stores/toast";
	import { confirmDialog } from "$lib/components/ui";
	import { Button } from "$lib/components/ui";
	import type {
		ExpenseRecord,
		CategoryTotal,
		BudgetLimit,
		PlannedPayment,
		ExpenseType,
	} from "$lib/api/types";

	import BudgetBars from "./expenses/BudgetBars.svelte";
	import ExpensePlansTable from "./expenses/ExpensePlansTable.svelte";
	import ExpenseLedgerTable from "./expenses/ExpenseLedgerTable.svelte";
	import ReceiptScanner from "./expenses/ReceiptScanner.svelte";
	import ExpenseForm from "./expenses/ExpenseForm.svelte";
	import ExpensePlanModal from "./expenses/ExpensePlanModal.svelte";
	import BudgetSettingsModal from "./expenses/BudgetSettingsModal.svelte";
	import ReceiptResultModal from "./expenses/ReceiptResultModal.svelte";
	import { todayIso, yen } from "./expenses/expenseUtils";

	// ── データ ──
	let expenses = $state<ExpenseRecord[]>([]);
	let breakdown = $state<CategoryTotal[]>([]);
	let limits = $state<BudgetLimit[]>([]);
	let plans = $state<PlannedPayment[]>([]);
	let monthTotal = $state(0);
	let incomeTotal = $state(0);

	// ── モーダル/UI 状態 ──
	let planModalOpen = $state(false);
	let budgetModalOpen = $state(false);
	let receiptModalOpen = $state(false);
	let receiptResponse = $state("");
	let scanning = $state(false);

	let expenseFormRef: ExpenseForm | undefined = $state();

	function reportError(e: unknown) {
		const msg = e instanceof ApiError ? e.message : "エラーが発生しました";
		pushToast(msg, "error");
	}

	// ── 支払い予定の集計（旧 expense-plans-due-count） ──
	const pendingPlans = $derived(plans.filter((p) => p.status === "pending"));
	const dueSoon = $derived(pendingPlans.filter((p) => p.due_date <= todayIso()));

	// ── 一覧・集計・予算・予定をまとめて取得（旧 fetchExpensesList の Promise.all） ──
	async function load() {
		try {
			const [exp, lim, pl] = await Promise.all([
				financeApi.list(),
				financeApi.budgetLimits(),
				financeApi.plans(),
			]);
			expenses = exp.expenses ?? [];
			breakdown = exp.breakdown ?? [];
			monthTotal = exp.total ?? 0;
			incomeTotal = exp.incomeTotal ?? 0;
			limits = lim.limits ?? [];
			plans = pl.plans ?? [];
		} catch (e) {
			reportError(e);
		}
	}

	$effect(() => {
		void $activeBot?.id;
		void load();
	});

	// ── 手動収支登録（旧 expenseForm submit） ──
	async function saveExpense(payload: {
		type: ExpenseType;
		amount: number;
		category: string;
		description: string;
		date: string;
		time: string;
	}) {
		try {
			await financeApi.add(payload);
			expenseFormRef?.reset();
			await load();
		} catch (e) {
			reportError(e);
		}
	}

	// ── 支払い予定 追加（旧 expensePlanForm submit） ──
	async function savePlan(payload: {
		title: string;
		amount: number;
		category: string;
		plannedDate: string;
		description: string;
	}) {
		try {
			await financeApi.addPlan(payload);
			planModalOpen = false;
			await load();
		} catch (e) {
			reportError(e);
		}
	}

	// ── 支払い予定 消込（旧 payBtn） ──
	async function onPayPlan(plan: PlannedPayment) {
		const ok = await confirmDialog({
			message: `「${plan.title}」${yen(plan.amount)} の支払いを完了しますか？\n家計簿に自動記録（消込）されます。`,
			confirmLabel: "支払う",
		});
		if (!ok) return;
		try {
			const res = await financeApi.payPlan(plan.id);
			if (res.message) pushToast(res.message, "success");
			await load();
		} catch (e) {
			reportError(e);
		}
	}

	// ── 支払い予定 キャンセル（旧 delBtn） ──
	async function onCancelPlan(plan: PlannedPayment) {
		const ok = await confirmDialog({
			message: `「${plan.title}」の支払い予定を取り消しますか？`,
			danger: true,
			confirmLabel: "予定を取り消す",
		});
		if (!ok) return;
		try {
			await financeApi.deletePlan(plan.id);
			await load();
		} catch (e) {
			reportError(e);
		}
	}

	// ── 予算上限 追加（旧 budgetLimitForm submit） ──
	async function onAddBudget(payload: { category: string; limitAmount: number }) {
		try {
			await financeApi.saveBudgetLimit(payload);
			const lim = await financeApi.budgetLimits();
			limits = lim.limits ?? [];
			await load();
		} catch (e) {
			reportError(e);
		}
	}

	// ── 予算上限 削除（旧 renderBudgetSettingsList の delBtn） ──
	async function onDeleteBudget(category: string) {
		try {
			await financeApi.deleteBudgetLimit(category);
			const lim = await financeApi.budgetLimits();
			limits = lim.limits ?? [];
			await load();
		} catch (e) {
			reportError(e);
		}
	}

	// ── レシートスキャン（旧 handleReceiptScan） ──
	async function onReceiptPick(file: File) {
		scanning = true;
		try {
			const { readReceiptFile } = await import("./expenses/expenseUtils");
			const { imageBase64, mimeType } = await readReceiptFile(file);
			const res = await financeApi.uploadReceipt({
				imageBase64,
				mimeType,
				additionalText: "WEB管理画面からアップロードされた画像レシート",
			});
			receiptResponse = res.response ?? "";
			receiptModalOpen = true;
			await load();
		} catch (e) {
			reportError(e);
		} finally {
			scanning = false;
		}
	}
</script>

<section class="tab-view">
	<!-- HUD: 当月合計・予算進捗 -->
	<div class="expense-hud-grid">
		<div class="hud-card card">
			<p>当月支出合計</p>
			<h2>{yen(monthTotal)}</h2>
			<div class="hud-card-sub hud-income">当月収入: {yen(incomeTotal)}</div>
			<div class="hud-card-sub" class:hud-due-warn={dueSoon.length > 0}>
				支払い予定: {pendingPlans.length}件{dueSoon.length > 0
					? ` (本日以前 ${dueSoon.length}件)`
					: ""}
			</div>
		</div>
		<div class="hud-card card col-span-2">
			<div class="budget-header">
				<p class="budget-header-title">カテゴリー別上限進捗</p>
				<Button variant="secondary" small onclick={() => (budgetModalOpen = true)}>
					<span class="material-symbols-outlined budget-header-icon" aria-hidden="true"
						>settings</span
					>上限設定
				</Button>
			</div>
			<div class="category-budget-bars">
				<BudgetBars {breakdown} {limits} />
			</div>
		</div>
	</div>

	<!-- 支払い予定 -->
	<div class="expense-table-card card expense-plans-card">
		<div class="column-header action-right">
			<h3>
				<span class="material-symbols-outlined header-icon-symbol" aria-hidden="true"
					>event_upcoming</span
				>支払い予定
			</h3>
			<Button variant="primary" small onclick={() => (planModalOpen = true)}
				>＋ 予定追加</Button
			>
		</div>
		<ExpensePlansTable {plans} onPay={onPayPlan} onCancel={onCancelPlan} />
	</div>

	<!-- レシートスキャナー ＆ 手動記録 -->
	<div class="expense-actions-columns">
		<ReceiptScanner {scanning} onpick={onReceiptPick} />
		<ExpenseForm bind:this={expenseFormRef} onsave={saveExpense} />
	</div>

	<!-- 直近の支出履歴 -->
	<div class="expense-table-card card">
		<div class="column-header">
			<h3>直近の支出履歴 (30件)</h3>
			<span class="hud-tag">LEDGER</span>
		</div>
		<ExpenseLedgerTable {expenses} />
	</div>
</section>

<ExpensePlanModal bind:open={planModalOpen} onsave={savePlan} />
<BudgetSettingsModal
	bind:open={budgetModalOpen}
	{limits}
	onadd={onAddBudget}
	ondelete={onDeleteBudget}
/>
<ReceiptResultModal bind:open={receiptModalOpen} response={receiptResponse} />

<style>
	.hud-income {
		color: var(--color-green);
	}
	.hud-due-warn {
		color: var(--color-red);
	}
	.budget-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-bottom: 10px;
	}
	.budget-header-title {
		margin: 0;
	}
	.budget-header-icon {
		font-size: 14px;
		vertical-align: middle;
	}
	.category-budget-bars {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.expense-plans-card {
		margin-bottom: 20px;
	}
</style>
