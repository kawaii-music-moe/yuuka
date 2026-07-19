<script lang="ts">
	// ─────────────────────────────────────────────────────────────────────────
	// ダッシュボード タブ（旧 app.js fetchDashboardStats(1634) / renderDonutChart /
	// renderPriceTrendChart / renderUsageChart(Axes) / renderUrgentDashboardList
	// + index.html #tab-dashboard を移植）。
	//
	// 2モード:
	//   - 秘書モード（既定）: /api/status + /api/expenses + 緊急リスト（予定/タスク）。
	//   - 汎用モード（mcp_assistant）: /api/bots/usage のAPI使用量ダッシュボード。
	// 手描きSVGは innerHTML 廃止し、{#each} で <path>/<line>/<circle> をバインド描画
	// （座標計算は dashboard/dashboardCharts.ts の純関数へ切り出し）。
	// activeBot（bot-scoped）変更で再取得。
	// ─────────────────────────────────────────────────────────────────────────
	import { activeBot } from "$lib/stores/activeBot";
	import { theme } from "$lib/stores/theme";
	import { settingsApi, financeApi, scheduleApi, taskApi, botApi } from "$lib/api/services";
	import { ApiError } from "$lib/api/client";
	import { pushToast } from "$lib/stores/toast";
	import type { ScheduleRecord, TodoWithSubtasks } from "$lib/api/types";

	import SecretaryDashboard from "./dashboard/SecretaryDashboard.svelte";
	import UsageDashboard from "./dashboard/UsageDashboard.svelte";
	import { secretaryBubble, assistantBubble, todayIso } from "./dashboard/dashboardUtils";
	import type {
		DashboardStats,
		DashboardExpense,
		DashboardExpenseBreakdown,
		UsageDashboardData,
	} from "./dashboard/dashboardTypes";

	// /api/status・/api/expenses の実レスポンス（型は汎用 or 他グループ所有なので局所拡張）。
	type StatusPayload = { stats?: DashboardStats };
	type ExpensesPayload = {
		expenses?: DashboardExpense[];
		total?: number;
		breakdown?: DashboardExpenseBreakdown[];
	};

	let bubble = $state("ロード中...");

	// 秘書モード state
	let stats = $state<DashboardStats | null>(null);
	let total = $state(0);
	let expenses = $state<DashboardExpense[]>([]);
	let breakdown = $state<DashboardExpenseBreakdown[]>([]);
	let schedules = $state<ScheduleRecord[]>([]);
	let pendingTasks = $state<TodoWithSubtasks[]>([]);

	// 汎用モード state
	let usage = $state<UsageDashboardData | null>(null);

	const isAssistant = $derived($activeBot?.preset === "mcp_assistant");
	const baTheme = $derived($theme === "blue-archive");

	// ウェルカムカードのアイコンは選択中Botのアバター（未設定Botは従来の固定画像へフォールバック）。
	const botAvatar = $derived($activeBot?.avatar || "/materials/yuka.webp");
	const botName = $derived($activeBot?.name || "アシスタント");

	function reportError(e: unknown) {
		const msg = e instanceof ApiError ? e.message : "エラーが発生しました";
		pushToast(msg, "error");
	}

	// ── 秘書モード集計（旧 fetchDashboardStats の秘書分岐） ──
	async function loadSecretary() {
		try {
			const statusRes = (await settingsApi.status()) as StatusPayload;
			const expenseRes = (await financeApi.list()) as ExpensesPayload & {
				success?: boolean;
			};

			stats = statusRes.stats ?? null;
			total = expenseRes.total ?? 0;
			expenses = expenseRes.expenses ?? [];
			breakdown = expenseRes.breakdown ?? [];

			if (stats) {
				bubble = secretaryBubble(total, stats.pendingTasks);
			}

			// 緊急リスト（今日の予定 + 未消化タスク）。旧 renderUrgentDashboardList。
			// scheduleApi は days パラメタ非対応（既定7日）なので今日ぶんを client 側で抽出。
			const today = todayIso();
			const [schedRes, taskRes] = await Promise.all([
				scheduleApi.list(),
				taskApi.list({ status: "pending" }),
			]);
			schedules = (schedRes.schedules ?? []).filter(
				(s) => s.start_at.slice(0, 10) === today,
			);
			pendingTasks = taskRes.tasks ?? [];
		} catch (e) {
			reportError(e);
			bubble =
				"ダッシュボード情報の取得中にエラーが発生しました。サーバーの接続状況を確認してください。";
		}
	}

	// ── 汎用モード集計（旧 fetchAssistantUsageDashboard） ──
	async function loadUsage(botId: string) {
		try {
			// /api/bots/usage は botId を手動 query 付与（scope:'user'）。
			// 実レスポンスは series/totals/days/rate_limits を含む（型は BotUsageResponse で
			// usage? のみのため局所型へキャスト）。
			const res = (await botApi.usage(botId)) as unknown as UsageDashboardData;
			usage = res;
			bubble = assistantBubble(
				res.series ?? [],
				res.totals ?? { requests: 0, responses: 0 },
				res.days ?? res.series?.length ?? 14,
			);
		} catch (e) {
			reportError(e);
			bubble = "API使用量の取得中にエラーが発生しました。サーバーの接続状況を確認してください。";
		}
	}

	// activeBot（bot-scoped）変更で、preset に応じたダッシュボードを再取得。
	$effect(() => {
		const bot = $activeBot;
		if (!bot?.id) return;
		if (bot.preset === "mcp_assistant") {
			void loadUsage(bot.id);
		} else {
			void loadSecretary();
		}
	});
</script>

<section class="tab-view">
	<!-- ウェルカム: アシスタント吹き出し -->
	<div class="welcome-card card">
		<div class="assistant-avatar-container">
			<img src={botAvatar} alt={botName} class="avatar-img" />
			<div class="avatar-badge">AI</div>
		</div>
		<div class="speech-bubble-container">
			<div class="character-name">{botName}</div>
			<div class="speech-bubble">
				<p>{bubble}</p>
			</div>
		</div>
	</div>

	{#if isAssistant}
		{#if usage}
			<UsageDashboard data={usage} />
		{/if}
	{:else if stats}
		<SecretaryDashboard
			{stats}
			{total}
			{expenses}
			{breakdown}
			{schedules}
			{pendingTasks}
			{baTheme}
		/>
	{/if}
</section>
