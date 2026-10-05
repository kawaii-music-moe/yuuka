<script lang="ts">
// 汎用モード（mcp_assistant）のAPI使用量ダッシュボード（旧 renderUsageDashboard 群）。
// 手描きSVGは innerHTML 廃止し {#each} で <path>/<line> をバインド描画。
import { Icon } from "$lib/components/ui";
import {
	usageChart,
	usageSummary,
	usageXAxis,
	usageYAxis,
} from "./dashboardCharts";
import type { UsageDashboardData } from "./dashboardTypes";
import { peakDateLabel } from "./dashboardUtils";

interface Props {
	data: UsageDashboardData;
}

let { data }: Props = $props();

const series = $derived(data.series ?? []);
const totals = $derived(data.totals ?? { requests: 0, responses: 0 });
const days = $derived(data.days ?? series.length ?? 14);
const summary = $derived(usageSummary(series, totals, days));
const chart = $derived(usageChart(series));
const yAxis = $derived(usageYAxis(series));
const xAxis = $derived(usageXAxis(series));

const rateLimits = $derived(data.rate_limits);
const rlItems = $derived(
	rateLimits
		? [
				{ label: "ユーザー1人 / 分", value: rateLimits.userPerMinute },
				{ label: "ユーザー1人 / 日", value: rateLimits.userPerDay },
				{ label: "サーバー1つ / 日", value: rateLimits.guildPerDay },
			]
		: [],
);

function fmtVal(v: number | undefined | null): string {
	return v === undefined || v === null ? "—" : v.toLocaleString();
}
</script>

<!-- 使用量サマリカード -->
<div class="stats-grid usage-stats-grid">
	<div class="stat-card card">
		<div class="stat-card-badge-trend">TODAY</div>
		<div class="stat-card-header"><span class="stat-label">本日のリクエスト</span></div>
		<div class="stat-value">{summary.today.toLocaleString()}</div>
		<div class="stat-footer-text">本日の受信メッセージ数</div>
	</div>
	<div class="stat-card card">
		<div class="stat-card-badge-trend">{days} DAYS</div>
		<div class="stat-card-header"><span class="stat-label">期間合計</span></div>
		<div class="stat-value">{summary.totalReq.toLocaleString()}</div>
		<div class="stat-footer-text">集計期間の総リクエスト数</div>
	</div>
	<div class="stat-card card">
		<div class="stat-card-badge-trend">AVG</div>
		<div class="stat-card-header"><span class="stat-label">日次平均</span></div>
		<div class="stat-value">{summary.avg.toLocaleString()}</div>
		<div class="stat-footer-text">1日あたりの平均リクエスト</div>
	</div>
	<div class="stat-card card">
		<div class="stat-card-badge-trend">PEAK</div>
		<div class="stat-card-header"><span class="stat-label">ピーク</span></div>
		<div class="stat-value">{summary.peak.toLocaleString()}</div>
		<div class="stat-footer-text">{peakDateLabel(summary.peakDate)}</div>
	</div>
</div>

<!-- 使用量推移チャート -->
<div class="dashboard-columns dashboard-columns-single">
	<div class="dashboard-column card">
		<div class="column-header">
			<h3><Icon name="monitoring" class="header-icon-symbol" />API使用量推移</h3>
			<div class="usage-chart-legend">
				<span class="usage-legend-item"><span class="usage-legend-dot usage-dot-req"></span>リクエスト</span>
				<span class="usage-legend-item"><span class="usage-legend-dot usage-dot-res"></span>応答</span>
				<span class="hud-tag">LIVE</span>
			</div>
		</div>
		<div class="chart-main-wrapper">
			<div class="chart-y-axis">
				{#each yAxis as v, i (i)}
					<span>{v}</span>
				{/each}
			</div>
			<div class="chart-body">
				<svg class="trend-chart-svg" viewBox="0 0 400 150" preserveAspectRatio="none">
					{#each chart.gridY as y, i (i)}
						<line class="usage-chart-grid-line" x1="0" y1={y} x2="400" y2={y} />
					{/each}
					{#if !chart.empty}
						<path class="usage-chart-req-area" d={chart.reqArea} />
						<path
							class="usage-chart-res-line"
							d={chart.resLine}
							vector-effect="non-scaling-stroke"
						/>
						<path
							class="usage-chart-req-line"
							d={chart.reqLine}
							vector-effect="non-scaling-stroke"
						/>
					{/if}
				</svg>
				<div class="chart-x-axis">
					{#each xAxis as label, i (i)}
						<span>{label}</span>
					{/each}
				</div>
			</div>
		</div>
		<div class="chart-sub-details">
			<div class="detail-item">
				<span class="det-label">合計応答数</span>
				<span class="det-value">{summary.totalRes.toLocaleString()}</span>
			</div>
			<div class="detail-item">
				<span class="det-label">応答率</span>
				<span class="det-value">{summary.responseRate}</span>
			</div>
			<div class="detail-item">
				<span class="det-label">集計期間</span>
				<span class="det-value">{days}日</span>
			</div>
		</div>
	</div>
</div>

<!-- レート制限 -->
<div class="full-width-urgent-row card">
	<div class="column-header">
		<h3><Icon name="speed" class="header-icon-symbol" />レート制限（現在の設定値）</h3>
		<span class="hud-pulse-dot"></span>
	</div>
	<div class="usage-ratelimit-grid">
		{#if rateLimits}
			{#each rlItems as it, i (i)}
				<div class="usage-ratelimit-item">
					<div class="usage-rl-value">{fmtVal(it.value)}</div>
					<div class="usage-rl-label">{it.label}</div>
				</div>
			{/each}
		{:else}
			<div class="usage-empty-note">レート制限の設定情報を取得できませんでした。</div>
		{/if}
	</div>
	<p class="usage-ratelimit-note">
		※ 上のリクエスト数は Bot 全体（全サーバー・全ユーザーの合算）の集計です。一方この上限は
		<strong>ユーザー1人ごと</strong>・<strong>サーバー1つごと</strong>に個別判定されるため、Bot
		全体の合計がこれらの数値を上回っていても、各ユーザー・各サーバーが上限内であれば正常に応答します。
		上限値の変更は管理者が「管理 → Bot属性設定」から行えます。
	</p>
</div>
