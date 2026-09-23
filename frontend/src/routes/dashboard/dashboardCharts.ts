// ダッシュボードSVGチャートの純関数群（DOM 非依存）。
// 旧 app.js の renderDonutChart / renderPriceTrendChart / renderUsageChart / renderUsageAxes /
// スパークライン描画の座標計算を、{#each} バインド用のデータ構造を返す形へ切り出す。
import type {
	DashboardExpense,
	DashboardExpenseBreakdown,
	UsageSeriesPoint,
} from "./dashboardTypes";

// ── 汎用フォーマッタ ──

/** 'YYYY-MM-DD' → 'M/D'（先頭ゼロなし。旧 formatMonthDay）。 */
export function formatMonthDay(dateStr: string): string {
	const parts = String(dateStr).split("-");
	if (parts.length < 3) return dateStr;
	return `${parseInt(parts[1], 10)}/${parseInt(parts[2], 10)}`;
}

/** ¥ 付き桁区切り。 */
export function yen(n: number): string {
	return `¥${Math.round(n).toLocaleString()}`;
}

// ── スパークライン（viewBox 0 0 100 20、5点）旧 schedules/expenses sparkline ──

/** trend 配列 → path d 文字列（x=idx*25, y=19-(v/max)*18）。 */
export function sparklinePath(trend: number[], minMax: number): string {
	const maxVal = Math.max(...trend, minMax);
	return trend
		.map((val, idx) => {
			const x = idx * 25;
			const y = 19 - (val / maxVal) * 18;
			return `${idx === 0 ? "M " : "L "}${x},${y.toFixed(2)}`;
		})
		.join(" ");
}

// ── 優先度ミニバー（旧 tasks-priority-bar-chart） ──

export interface PriorityBar {
	label: string;
	count: number;
	/** 高さ%（最低10%）。 */
	heightPct: number;
	color: string;
}

/**
 * pendingPriorities（0/1/2 キー）→ バー3本。
 * baColors=true で BA テーマ配色。
 */
export function priorityBars(
	priorities: Record<number, number>,
	baTheme: boolean,
): PriorityBar[] {
	const p0 = priorities[0] ?? 0;
	const p1 = priorities[1] ?? 0;
	const p2 = priorities[2] ?? 0;
	const maxCount = Math.max(p0, p1, p2, 1);
	const colors = baTheme
		? ["#B8E8F8", "#51C8E8", "#02D3FB"]
		: ["#4f545c", "#fbbf24", "#da373c"];
	const labels = ["低", "中", "高"];
	return [p0, p1, p2].map((count, i) => ({
		label: labels[i],
		count,
		heightPct: Math.max((count / maxCount) * 100, 10),
		color: colors[i],
	}));
}

// ── 経費推移ラインチャート（viewBox 0 0 400 150、過去6日） 旧 renderPriceTrendChart ──

export interface TrendPoint {
	x: number;
	y: number;
	amount: number;
}
export interface PriceTrendChart {
	points: TrendPoint[];
	linePath: string;
	areaPath: string;
	/** X軸ラベル（5日前/昨日/今日/M/D）。 */
	labels: string[];
}

/** 直近6日（5日前〜今日）の日次支出合計から line/area path と点を計算する。 */
export function priceTrendChart(
	expenses: DashboardExpense[] | undefined,
): PriceTrendChart {
	const dateStrings: string[] = [];
	const dateLabels: string[] = [];
	for (let i = 5; i >= 0; i--) {
		const d = new Date();
		d.setDate(d.getDate() - i);
		dateStrings.push(d.toISOString().slice(0, 10));
		dateLabels.push(`${d.getMonth() + 1}/${d.getDate()}`);
	}
	const labels = dateLabels.map((label, idx) =>
		idx === 0 ? "5日前" : idx === 4 ? "昨日" : idx === 5 ? "今日" : label,
	);

	const dailyTotals = dateStrings.map((date) =>
		(expenses ?? [])
			.filter((e) => e.date === date)
			.reduce((sum, e) => sum + e.amount, 0),
	);
	const maxVal = Math.max(...dailyTotals, 10000);

	const points: TrendPoint[] = dailyTotals.map((val, idx) => ({
		x: idx * 80,
		y: 130 - (val / maxVal) * 100,
		amount: val,
	}));

	const linePath = points
		.map((p, idx) => `${idx === 0 ? "M" : "L"} ${p.x},${p.y.toFixed(2)}`)
		.join(" ");
	const areaPath = `M 0,130 ${points
		.map((p) => `L ${p.x},${p.y.toFixed(2)}`)
		.join(" ")} L 400,130 Z`;

	return { points, linePath, areaPath, labels };
}

// ── ドーナツチャート（娯楽比率） 旧 renderDonutChart ──

export interface DonutLegendItem {
	label: string;
	color: string;
}
export interface DonutChart {
	/** stroke-dasharray（娯楽割合ぶん / 全周 251.2）。 */
	dashArray: string;
	/** 中央%表示。 */
	centerPercent: string;
	legend: DonutLegendItem[];
	empty: boolean;
}

export function donutChart(
	breakdown: DashboardExpenseBreakdown[] | undefined,
	total: number,
	baTheme: boolean,
): DonutChart {
	if (!breakdown || breakdown.length === 0 || total === 0) {
		return {
			dashArray: "0 251.2",
			centerPercent: "0%",
			legend: [],
			empty: true,
		};
	}

	const entertainment = breakdown.find((c) => c.category === "娯楽");
	const entPct = entertainment
		? Math.round((entertainment.total / total) * 100)
		: 0;
	const strokeDash = (entPct * 251.2) / 100;

	const colorMap: Record<string, string> = baTheme
		? {
				食費: "#02D3FB",
				日用品: "#A3BAFF",
				交通費: "#FB90A4",
				娯楽: "#FFD966",
				その他: "#7A9BB0",
			}
		: {
				食費: "#5865F2",
				日用品: "#248046",
				交通費: "#fbbf24",
				娯楽: "#f472b6",
				その他: "#4f545c",
			};

	const legend: DonutLegendItem[] = breakdown.slice(0, 4).map((cat) => {
		const pct = Math.round((cat.total / total) * 100);
		return {
			label: `${cat.category}: ${yen(cat.total)} (${pct}%)`,
			color: colorMap[cat.category] || "#a78bfa",
		};
	});

	return {
		dashArray: `${strokeDash.toFixed(2)} 251.2`,
		centerPercent: `${entPct}%`,
		legend,
		empty: false,
	};
}

// ── API使用量チャート（viewBox 0 0 400 150、preserveAspectRatio=none） 旧 renderUsageChart ──

export interface UsageChart {
	reqLine: string;
	resLine: string;
	reqArea: string;
	/** 水平グリッド線の y 座標（3本）。 */
	gridY: number[];
	empty: boolean;
}

export function usageChart(series: UsageSeriesPoint[]): UsageChart {
	const W = 400;
	const H = 150;
	const padTop = 12;
	const padBottom = 8;
	const innerH = H - padTop - padBottom;
	const n = series.length;

	if (n === 0) {
		return { reqLine: "", resLine: "", reqArea: "", gridY: [], empty: true };
	}

	const maxVal = Math.max(
		1,
		...series.map((s) => Math.max(s.requests, s.responses)),
	);
	const xAt = (i: number) => (n <= 1 ? W / 2 : (i / (n - 1)) * W);
	const yAt = (v: number) => padTop + innerH - (v / maxVal) * innerH;
	const toLine = (pts: [number, number][]) =>
		pts
			.map(
				(p, i) => `${i === 0 ? "M" : "L"}${p[0].toFixed(1)},${p[1].toFixed(1)}`,
			)
			.join(" ");

	const reqPts: [number, number][] = series.map((s, i) => [
		xAt(i),
		yAt(s.requests),
	]);
	const resPts: [number, number][] = series.map((s, i) => [
		xAt(i),
		yAt(s.responses),
	]);
	const reqLine = toLine(reqPts);
	const resLine = toLine(resPts);
	const baseY = padTop + innerH;
	const reqArea = `${reqLine} L${xAt(n - 1).toFixed(1)},${baseY.toFixed(1)} L${xAt(0).toFixed(1)},${baseY.toFixed(1)} Z`;

	const gridY = [0.25, 0.5, 0.75].map((f) => padTop + innerH - f * innerH);

	return { reqLine, resLine, reqArea, gridY, empty: false };
}

/** 使用量チャートのY軸目盛（max, 2/3, 1/3, 0）。旧 renderUsageAxes。 */
export function usageYAxis(series: UsageSeriesPoint[]): number[] {
	const maxVal = Math.max(
		1,
		...series.map((s) => Math.max(s.requests, s.responses)),
	);
	return [maxVal, Math.round((maxVal * 2) / 3), Math.round(maxVal / 3), 0];
}

/** 使用量チャートのX軸日付ラベル（最大5個、等間隔）。旧 renderUsageAxes。 */
export function usageXAxis(series: UsageSeriesPoint[]): string[] {
	const n = series.length;
	if (n === 0) return [];
	const labelCount = Math.min(5, n);
	const idxs: number[] = [];
	for (let k = 0; k < labelCount; k++) {
		idxs.push(Math.round((k / (labelCount - 1 || 1)) * (n - 1)));
	}
	return [...new Set(idxs)].map((i) => formatMonthDay(series[i].date));
}

// ── 使用量サマリ集計（旧 renderUsageDashboard の数値算出部） ──

export interface UsageSummary {
	today: number;
	totalReq: number;
	totalRes: number;
	avg: number;
	peak: number;
	peakDate: string;
	days: number;
	responseRate: string;
}

export function usageSummary(
	series: UsageSeriesPoint[],
	totals: { requests: number; responses: number },
	days: number,
): UsageSummary {
	const today = series.length ? series[series.length - 1].requests : 0;
	const totalReq = totals.requests || 0;
	const totalRes = totals.responses || 0;
	const avg = series.length ? Math.round(totalReq / series.length) : 0;
	let peak = 0;
	let peakDate = "";
	for (const s of series) {
		if (s.requests > peak) {
			peak = s.requests;
			peakDate = s.date;
		}
	}
	return {
		today,
		totalReq,
		totalRes,
		avg,
		peak,
		peakDate,
		days,
		responseRate:
			totalReq > 0 ? `${Math.round((totalRes / totalReq) * 100)}%` : "—",
	};
}
