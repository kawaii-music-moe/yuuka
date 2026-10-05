<script lang="ts">
// ガントチャート（旧 app.js:3240 renderGantt）。
// chart.js を動的 import（初回描画時のみバンドルを取得）し、水平バーで
// 開始日〜期限のレンジを描く。進捗塗り + 今日の縦線はカスタムプラグイン。
// onDestroy で Chart インスタンスを破棄する。

import type { Chart as ChartType, Plugin } from "chart.js";
import { onDestroy } from "svelte";
import type { TodoWithSubtasks } from "$lib/api/types";
import { EmptyState } from "$lib/components/ui";
import {
	buildGanttRows,
	DAY_MS,
	type GanttRow,
	startOfTodayMs,
} from "./taskUtils";

interface Props {
	tasks: TodoWithSubtasks[];
}

let { tasks }: Props = $props();

let canvas = $state<HTMLCanvasElement | null>(null);
let chart: ChartType | null = null;

const rows = $derived(buildGanttRows(tasks));
// 行数に応じた高さ確保（旧 scroll.style.height）。
const scrollHeight = $derived(`${rows.length * 36 + 70}px`);

function destroyChart() {
	if (chart) {
		chart.destroy();
		chart = null;
	}
}

async function render(currentRows: GanttRow[]) {
	destroyChart();
	if (!canvas || currentRows.length === 0) return;

	// chart.js を動的 import（唯一の利用箇所。バンドルの遅延ロード）。
	const { Chart, registerables } = await import("chart.js");
	Chart.register(...registerables);

	// 描画中に tasks が変わって canvas が消えている場合は中断。
	if (!canvas) return;

	const minMs = Math.min(
		...currentRows.map((r) => r.range[0]),
		startOfTodayMs(),
	);
	const maxMs = Math.max(
		...currentRows.map((r) => r.range[1]),
		startOfTodayMs(),
	);
	const pad = Math.max(DAY_MS, (maxMs - minMs) * 0.05);

	// バー内に進捗分の塗りを重ねるプラグイン（旧 progressFill）。
	const progressFill: Plugin<"bar"> = {
		id: "ganttProgressFill",
		afterDatasetsDraw(c) {
			const ctx = c.ctx;
			const meta = c.getDatasetMeta(0);
			const progress =
				(c.data.datasets[0] as { progress?: number[] }).progress || [];
			meta.data.forEach((bar, i) => {
				const pct = progress[i] || 0;
				if (!pct) return;
				const props = bar.getProps(["x", "base", "y", "height"], true) as {
					x: number;
					base: number;
					y: number;
					height: number;
				};
				const left = Math.min(props.base, props.x);
				const width = Math.abs(props.x - props.base);
				ctx.save();
				ctx.fillStyle = "rgba(255,255,255,0.4)";
				ctx.fillRect(
					left,
					props.y - props.height / 2,
					width * (pct / 100),
					props.height,
				);
				ctx.restore();
			});
		},
	};

	// 今日の縦線（旧 todayLine）。
	const todayLine: Plugin<"bar"> = {
		id: "ganttTodayLine",
		afterDraw(c) {
			const x = c.scales.x;
			const today = startOfTodayMs();
			if (today < x.min || today > x.max) return;
			const px = x.getPixelForValue(today);
			const { top, bottom } = c.chartArea;
			const ctx = c.ctx;
			ctx.save();
			ctx.strokeStyle = "rgba(237,66,69,0.9)";
			ctx.lineWidth = 1.5;
			ctx.setLineDash([4, 3]);
			ctx.beginPath();
			ctx.moveTo(px, top);
			ctx.lineTo(px, bottom);
			ctx.stroke();
			ctx.restore();
		},
	};

	chart = new Chart(canvas, {
		type: "bar",
		data: {
			labels: currentRows.map((r) => r.label),
			datasets: [
				{
					data: currentRows.map((r) => r.range),
					backgroundColor: currentRows.map((r) => r.color),
					borderColor: "rgba(187,134,252,0.9)",
					borderWidth: 1,
					borderSkipped: false,
					borderRadius: 4,
					// カスタムプラグインが読む拡張プロパティ。
					progress: currentRows.map((r) => r.progress),
				} as unknown as ChartType["data"]["datasets"][number],
			],
		},
		options: {
			indexAxis: "y",
			responsive: true,
			maintainAspectRatio: false,
			scales: {
				x: {
					type: "linear",
					position: "top",
					min: minMs - pad,
					max: maxMs + pad,
					ticks: {
						callback: (v) => {
							const d = new Date(Number(v));
							return `${d.getMonth() + 1}/${d.getDate()}`;
						},
						color: "rgba(160,160,170,0.9)",
						maxRotation: 0,
					},
					grid: { color: "rgba(128,128,128,0.15)" },
				},
				y: {
					ticks: { color: "rgba(200,200,210,0.95)", font: { size: 11 } },
					grid: { display: false },
				},
			},
			plugins: {
				legend: { display: false },
				tooltip: {
					callbacks: {
						label: (item) => {
							const r = item.raw as [number, number];
							const s = new Date(r[0]);
							const e = new Date(r[1]);
							const pct = currentRows[item.dataIndex].progress;
							return `${s.getMonth() + 1}/${s.getDate()} 〜 ${e.getMonth() + 1}/${e.getDate()}  進捗${pct}%`;
						},
					},
				},
			},
		},
		plugins: [progressFill, todayLine],
	});
}

// tasks（→rows）と canvas の準備が整うたびに再描画。
$effect(() => {
	void render(rows);
});

onDestroy(destroyChart);
</script>

<div class="card">
	{#if rows.length === 0}
		<EmptyState
			icon="calendar_view_week"
			message="ガントに表示できるタスクがありません。タスクに開始日か期限を設定すると、ここに表示されます（日付のないタスクは「いつかやる」に並びます）。"
		/>
	{:else}
		<div class="gantt-scroll" style="height:{scrollHeight}">
			<canvas bind:this={canvas}></canvas>
		</div>
	{/if}
</div>

<style>
	.gantt-scroll {
		overflow-x: auto;
	}
</style>
