<script lang="ts">
// 秘書モードのダッシュボード本体（旧 dashboard-secretary-only 群）。
// 手描きSVGは innerHTML 廃止し {#each} で <path>/<line>/<circle> をバインド描画。

import type { ScheduleRecord, TodoWithSubtasks } from "$lib/api/types";
import { Icon } from "$lib/components/ui";
import {
	donutChart,
	priceTrendChart,
	priorityBars,
	sparklinePath,
	yen,
} from "./dashboardCharts";
import type {
	DashboardExpense,
	DashboardExpenseBreakdown,
	DashboardStats,
} from "./dashboardTypes";
import {
	averageExpense,
	buildUrgentItems,
	highestCategory,
	highestExpense,
} from "./dashboardUtils";

interface Props {
	stats: DashboardStats;
	total: number;
	expenses: DashboardExpense[];
	breakdown: DashboardExpenseBreakdown[];
	schedules: ScheduleRecord[];
	pendingTasks: TodoWithSubtasks[];
	baTheme: boolean;
}

let {
	stats,
	total,
	expenses,
	breakdown,
	schedules,
	pendingTasks,
	baTheme,
}: Props = $props();

const scheduleSpark = $derived(
	sparklinePath(stats.scheduleTrend ?? [0, 0, 0, 0, 0], 2),
);
const expenseSpark = $derived(
	sparklinePath(stats.expenseTrend ?? [0, 0, 0, 0, 0], 5000),
);
const bars = $derived(priorityBars(stats.pendingPriorities ?? {}, baTheme));
const trend = $derived(priceTrendChart(expenses));
const donut = $derived(donutChart(breakdown, total, baTheme));
const urgent = $derived(buildUrgentItems(schedules, pendingTasks));
const highExpense = $derived(highestExpense(expenses));
const highCategory = $derived(highestCategory(breakdown));
const avgExpense = $derived(averageExpense(total, expenses));
</script>

<!-- スタッツカード -->
<div class="stats-grid">
	<!-- タスク -->
	<div class="stat-card card">
		<div class="stat-card-badge-trend">ACTIVE</div>
		<div class="stat-card-header"><span class="stat-label">ACTIVE TASKS</span></div>
		<div class="stat-value">{stats.pendingTasks}</div>
		<div class="stat-sparkline priority-bars">
			{#each bars as bar (bar.label)}
				<div
					class="priority-bar-col"
					title="{bar.label}優先度: {bar.count}件"
				>
					<span class="priority-bar-count">{bar.count}</span>
					<div
						class="priority-bar"
						style="height:{bar.heightPct}%;background-color:{bar.color};"
					></div>
				</div>
			{/each}
		</div>
		<div class="stat-footer-text">後で困らないよう、早めに処理を！</div>
	</div>

	<!-- スケジュール -->
	<div class="stat-card card">
		<div class="stat-card-badge-trend">7 DAYS</div>
		<div class="stat-card-header"><span class="stat-label">UPCOMING EVENTS</span></div>
		<div class="stat-value">{stats.schedules}</div>
		<div class="stat-sparkline">
			<svg class="sparkline-svg" viewBox="0 0 100 20">
				<path d={scheduleSpark} fill="none" stroke="#e4e4e7" stroke-width="1.5" />
			</svg>
		</div>
		<div class="stat-footer-text">時間は有限です。計画的な予定を。</div>
	</div>

	<!-- 経費 -->
	<div class="stat-card card">
		<div class="stat-card-badge-trend">30 DAYS</div>
		<div class="stat-card-header"><span class="stat-label">SEMINAR BUDGET</span></div>
		<div class="stat-value">{yen(total)}</div>
		<div class="stat-sparkline">
			<svg class="sparkline-svg" viewBox="0 0 100 20">
				<path d={expenseSpark} fill="none" stroke="#fafafa" stroke-width="1.5" />
			</svg>
		</div>
		<div class="stat-footer-text">計算の狂いは、即ち家計の乱れです！</div>
	</div>
</div>

<!-- チャート2カラム -->
<div class="dashboard-columns">
	<!-- 経費推移ライン -->
	<div class="dashboard-column card">
		<div class="column-header">
			<h3><Icon name="trending_up" class="header-icon-symbol" />経費支出推移 (財務チャート)</h3>
			<span class="hud-tag">LIVE</span>
		</div>
		<div class="chart-main-wrapper">
			<div class="chart-y-axis">
				<span>50k</span><span>30k</span><span>10k</span><span>0k</span>
			</div>
			<div class="chart-body">
				<svg class="trend-chart-svg" viewBox="0 0 400 150">
					<line x1="0" y1="20" x2="400" y2="20" stroke="#27272a" stroke-width="0.7" stroke-dasharray="3,3" />
					<line x1="0" y1="60" x2="400" y2="60" stroke="#27272a" stroke-width="0.7" stroke-dasharray="3,3" />
					<line x1="0" y1="100" x2="400" y2="100" stroke="#27272a" stroke-width="0.7" stroke-dasharray="3,3" />
					<line x1="0" y1="130" x2="400" y2="130" stroke="#27272a" stroke-width="0.7" stroke-dasharray="3,3" />
					<path d={trend.areaPath} fill="transparent" />
					<path
						d={trend.linePath}
						fill="none"
						stroke="#fafafa"
						stroke-width="2.2"
						stroke-linecap="round"
					/>
					{#each trend.points as p, i (i)}
						<circle
							cx={p.x}
							cy={p.y}
							r="4.5"
							fill="var(--color-primary)"
							stroke="var(--bg-primary)"
							stroke-width="1.5"
						/>
					{/each}
				</svg>
				<div class="chart-x-axis">
					{#each trend.labels as label, i (i)}
						<span>{label}</span>
					{/each}
				</div>
			</div>
		</div>
		<div class="chart-sub-details">
			<div class="detail-item">
				<span class="det-label">最大単発支出</span>
				<span class="det-value">{yen(highExpense)}</span>
			</div>
			<div class="detail-item">
				<span class="det-label">主要カテゴリ</span>
				<span class="det-value">{highCategory}</span>
			</div>
			<div class="detail-item">
				<span class="det-label">日次平均</span>
				<span class="det-value">{yen(avgExpense)}</span>
			</div>
		</div>
	</div>

	<!-- カテゴリ別ドーナツ -->
	<div class="dashboard-column card">
		<div class="column-header">
			<h3><Icon name="pie_chart" class="header-icon-symbol" />カテゴリ別</h3>
		</div>
		<div class="chart-container-flex">
			<div class="pie-chart-wrapper">
				<svg class="donut-chart" viewBox="0 0 100 100">
					<circle cx="50" cy="50" r="40" fill="transparent" stroke="#27272a" stroke-width="9" />
					<circle
						cx="50"
						cy="50"
						r="40"
						fill="transparent"
						stroke="#fafafa"
						stroke-width="9"
						stroke-dasharray={donut.dashArray}
						stroke-dashoffset="0"
						stroke-linecap="round"
					/>
				</svg>
				<div class="chart-center-label">
					<div>{donut.centerPercent}</div>
					<div class="chart-center-sub">娯楽比率</div>
				</div>
			</div>
			<div class="chart-legend">
				{#if donut.empty}
					<div class="legend-item">今月のデータはありません。</div>
				{:else}
					{#each donut.legend as item, i (i)}
						<div class="legend-item">
							<span class="legend-color" style="background-color:{item.color};"></span>
							<span>{item.label}</span>
						</div>
					{/each}
				{/if}
			</div>
		</div>
	</div>
</div>

<!-- 緊急予定 & 滞留タスク -->
<div class="full-width-urgent-row card">
	<div class="column-header">
		<h3><Icon name="bolt" class="header-icon-symbol" />今日の緊急予定 ＆ 滞留タスク</h3>
		<span class="hud-pulse-dot"></span>
	</div>
	<div class="dashboard-list">
		{#if urgent.length > 0}
			{#each urgent as item, i (i)}
				<div class="urgent-item">
					<Icon name={item.icon} class="icon-small urgent-item-icon" />
					<span class="urgent-item-label">{item.label}</span>
					<span class="urgent-badge {item.badgeClass}">{item.badge}</span>
				</div>
			{/each}
		{:else}
			<div class="urgent-item">
				今日の急ぎのタスクや予定はありません！素晴らしい計画性ですね！
			</div>
		{/if}
	</div>
</div>

<style>
	.priority-bars {
		display: flex;
		align-items: flex-end;
		gap: 8px;
		height: 28px;
		margin-bottom: 8px;
	}
	.priority-bar-col {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: flex-end;
		flex: 1;
		height: 100%;
	}
	.priority-bar-count {
		font-size: 0.55rem;
		font-family: var(--font-family-mono);
		color: var(--color-zinc-muted);
		margin-bottom: 2px;
	}
	.priority-bar {
		width: 100%;
		border-radius: var(--radius);
		transition: height 0.3s ease;
	}
	.urgent-item-label {
		font-weight: bold;
	}
	:global(.urgent-item-icon) {
		margin-right: 6px;
	}
</style>
