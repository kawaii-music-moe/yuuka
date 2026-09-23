<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// タイムライン タブ（旧 app.js デイリータイムライン群 2811-3187 + index.html
// #tab-timeline を移植）。timelineApi 使用（bot-scoped）。
//
// 挙動の忠実移植:
//   - 日付ナビ（前/次/今日、tlShiftDay）。日付変更で再取得。
//   - 2カラム（計画 blocks / 記録 records）を並べて表示。
//   - 計画ブロック 追加/編集/削除、記録 追加/削除。
//   - メディア（写真・動画）は base64 化して /api/timeline/media へ。
//   - activeBot 変更でリロード。
// ─────────────────────────────────────────────────────────────────────────

import { ApiError } from "$lib/api/client";
import { timelineApi } from "$lib/api/services";
import type { DayPlanBlock, TimelineRecord } from "$lib/api/types";
import { confirmDialog, Icon } from "$lib/components/ui";
import { activeBot } from "$lib/stores/activeBot";
import { pushToast } from "$lib/stores/toast";
import PlanBlockModal, {
	type PlanBlockFormPayload,
} from "./timeline/PlanBlockModal.svelte";
import PlanCard from "./timeline/PlanCard.svelte";
import RecordCard from "./timeline/RecordCard.svelte";
import RecordModal, {
	type RecordFormPayload,
} from "./timeline/RecordModal.svelte";
import {
	fileToBase64,
	fmtTimelineDate,
	shiftDay,
	todayIso,
} from "./timeline/timelineUtils";

let currentDate = $state(todayIso());
let blocks = $state<DayPlanBlock[]>([]);
let records = $state<TimelineRecord[]>([]);

// モーダル state
let planOpen = $state(false);
let editingBlock = $state<DayPlanBlock | null>(null);
let recordOpen = $state(false);

const dateLabel = $derived(fmtTimelineDate(currentDate));

function reportError(e: unknown) {
	const msg = e instanceof ApiError ? e.message : "エラーが発生しました";
	pushToast(msg, "error");
}

async function loadDay() {
	try {
		const res = await timelineApi.day(currentDate);
		blocks = res.blocks ?? [];
		records = res.records ?? [];
	} catch (e) {
		reportError(e);
		blocks = [];
		records = [];
	}
}

// activeBot（bot-scoped）変更 or currentDate 変更で再取得。
$effect(() => {
	void $activeBot?.id;
	void currentDate;
	void loadDay();
});

// ── 日付ナビ ──
function prevDay() {
	currentDate = shiftDay(currentDate, -1);
}
function nextDay() {
	currentDate = shiftDay(currentDate, 1);
}
function goToday() {
	currentDate = todayIso();
}

// ── 計画ブロック ──
function openNewPlan() {
	editingBlock = null;
	planOpen = true;
}
function onEditPlan(block: DayPlanBlock) {
	editingBlock = block;
	planOpen = true;
}
async function savePlan(p: PlanBlockFormPayload) {
	try {
		const common = {
			date: currentDate,
			type: p.type,
			title: p.title,
			startTime: p.startTime || undefined,
			endTime: p.endTime || undefined,
			description: p.description || undefined,
			transitFrom: p.transitFrom || undefined,
			transitTo: p.transitTo || undefined,
			transitLine: p.transitLine || undefined,
		};
		if (p.id != null) {
			await timelineApi.updatePlan({ ...common, id: p.id });
		} else {
			await timelineApi.addPlan(common);
		}
		planOpen = false;
		await loadDay();
	} catch (e) {
		reportError(e);
	}
}
async function onDeletePlan(id: number) {
	const ok = await confirmDialog({
		message: "この計画ブロックを削除しますか？",
		danger: true,
		confirmLabel: "削除",
	});
	if (!ok) return;
	try {
		await timelineApi.deletePlan(id);
		await loadDay();
	} catch (e) {
		reportError(e);
	}
}

// ── 記録 ──
function openNewRecord() {
	recordOpen = true;
}
async function saveRecord(p: RecordFormPayload) {
	try {
		if (p.kind === "media") {
			const base64 = await fileToBase64(p.file);
			await timelineApi.uploadMedia({
				date: currentDate,
				base64,
				mimeType: p.file.type,
				title: p.title || undefined,
				location: p.location || undefined,
			});
		} else {
			await timelineApi.addRecord({
				date: currentDate,
				type: p.type,
				title: p.title || undefined,
				content: p.content || undefined,
				location: p.location || undefined,
				...(p.type === "expense"
					? { amount: p.amount, category: p.category }
					: {}),
			});
		}
		recordOpen = false;
		await loadDay();
	} catch (e) {
		reportError(e);
	}
}
async function onDeleteRecord(id: number) {
	const ok = await confirmDialog({
		message: "この記録を削除しますか？",
		danger: true,
		confirmLabel: "削除",
	});
	if (!ok) return;
	try {
		await timelineApi.deleteRecord(id);
		await loadDay();
	} catch (e) {
		reportError(e);
	}
}
</script>

<section class="tab-view">
	<!-- 日付ナビゲーション -->
	<div class="tl-header card">
		<div class="tl-date-nav">
			<button type="button" class="btn-icon" aria-label="前の日" onclick={prevDay}>
				<Icon name="chevron_left" />
			</button>
			<span class="tl-date-label">{dateLabel}</span>
			<button type="button" class="btn-icon" aria-label="次の日" onclick={nextDay}>
				<Icon name="chevron_right" />
			</button>
			<button type="button" class="btn btn-secondary btn-sm" onclick={goToday}
				>今日</button
			>
		</div>
		<div class="tl-header-actions">
			<button type="button" class="btn btn-secondary" onclick={openNewPlan}>
				<Icon name="add_box" /> 計画追加
			</button>
			<button type="button" class="btn btn-primary" onclick={openNewRecord}>
				<Icon name="edit_note" /> 記録追加
			</button>
		</div>
	</div>

	<!-- 2カラム: 計画 / 記録 -->
	<div class="tl-body">
		<div class="tl-col">
			<div class="tl-col-header">
				<Icon name="event_available" /> 計画
			</div>
			<div class="tl-list">
				{#if blocks.length > 0}
					{#each blocks as block (block.id)}
						<PlanCard {block} onedit={onEditPlan} ondelete={onDeletePlan} />
					{/each}
				{:else}
					<div class="tl-empty">計画はまだありません</div>
				{/if}
			</div>
		</div>
		<div class="tl-col">
			<div class="tl-col-header">
				<Icon name="history" /> 記録
			</div>
			<div class="tl-list">
				{#if records.length > 0}
					{#each records as record (record.id)}
						<RecordCard {record} ondelete={onDeleteRecord} />
					{/each}
				{:else}
					<div class="tl-empty">記録はまだありません</div>
				{/if}
			</div>
		</div>
	</div>
</section>

<PlanBlockModal bind:open={planOpen} {editingBlock} onsave={savePlan} />
<RecordModal bind:open={recordOpen} onsave={saveRecord} />
