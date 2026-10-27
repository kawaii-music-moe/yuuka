<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// タイムライン タブ（旧 app.js デイリータイムライン群 2811-3187 + index.html
// #tab-timeline を移植）。timelineApi 使用（bot-scoped）。
//
// 挙動の忠実移植:
//   - 日付ナビ（前/次/今日）。前/次は計画か記録のある最寄りの日へ飛び、無い方向は非活性。
//     日付変更で再取得。
//   - 計画 blocks / 記録 records はタブで切り替えて表示（タブはシェルのヘッダーに出す）。
//   - 計画ブロック 追加/編集/削除、記録 追加/削除。
//   - メディア（写真・動画）は base64 化して /api/timeline/media へ。
//   - activeBot 変更でリロード。
// ─────────────────────────────────────────────────────────────────────────

import { onDestroy } from "svelte";
import { ApiError } from "$lib/api/client";
import { timelineApi } from "$lib/api/services";
import type { DayPlanBlock, TimelineRecord } from "$lib/api/types";
import { confirmDialog, Icon, PageTabs } from "$lib/components/ui";
import { activeBot } from "$lib/stores/activeBot";
import { headerExtra } from "$lib/stores/headerExtra";
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
	todayIso,
} from "./timeline/timelineUtils";

const VIEWS = [
	{ id: "plan", label: "計画", icon: "event_available" },
	{ id: "record", label: "記録", icon: "history" },
];
let view = $state("plan");

let currentDate = $state(todayIso());
let blocks = $state<DayPlanBlock[]>([]);
let records = $state<TimelineRecord[]>([]);
// 前後で計画か記録のある最寄りの日付（無い方向は null = ボタン非活性）。
let prevDate = $state<string | null>(null);
let nextDate = $state<string | null>(null);

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

// 基準日そのものは含まないので、その日の追加・削除では変わらない（日付・Bot の変更時だけ取る）。
async function loadNeighbors(date: string) {
	prevDate = null;
	nextDate = null;
	try {
		const res = await timelineApi.neighbors(date);
		// 連打で日付が先に進んでいたら古い応答は捨てる。
		if (date !== currentDate) return;
		prevDate = res.prev ?? null;
		nextDate = res.next ?? null;
	} catch (e) {
		reportError(e);
	}
}

// activeBot（bot-scoped）変更 or currentDate 変更で再取得。
$effect(() => {
	void $activeBot?.id;
	void loadDay();
	void loadNeighbors(currentDate);
});

// ── 日付ナビ ──
function prevDay() {
	if (prevDate) currentDate = prevDate;
}
function nextDay() {
	if (nextDate) currentDate = nextDate;
}
function goToday() {
	currentDate = todayIso();
}
function pickDate(e: Event & { currentTarget: HTMLInputElement }) {
	if (e.currentTarget.value) currentDate = e.currentTarget.value;
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

// タブはシェルのヘッダーに出す（Playbook / Webhook ページと同じ）。
headerExtra.set(headerTabs);
onDestroy(() => headerExtra.set(null));
</script>

{#snippet headerTabs()}
	<PageTabs tabs={VIEWS} bind:active={view} label="タイムラインの表示" flush />
{/snippet}

<section class="tab-view">
	<!-- 日付ナビゲーション -->
	<div class="tl-header card">
		<div class="tl-date-nav">
			<button
				type="button"
				class="btn-icon"
				aria-label="前の日"
				title={prevDate ? undefined : "これより前の計画・記録はありません"}
				disabled={!prevDate}
				onclick={prevDay}
			>
				<Icon name="chevron_left" />
			</button>
			<!-- 前後ボタンはデータのある日にしか飛ばないので、空の日（明日など）へはここから行く -->
			<label class="tl-date-label tl-date-pick" title="日付を選ぶ">
				{dateLabel}
				<input
					type="date"
					aria-label="日付を選ぶ"
					value={currentDate}
					onchange={pickDate}
				/>
			</label>
			<button
				type="button"
				class="btn-icon"
				aria-label="次の日"
				title={nextDate ? undefined : "これより後の計画・記録はありません"}
				disabled={!nextDate}
				onclick={nextDay}
			>
				<Icon name="chevron_right" />
			</button>
			<button type="button" class="btn btn-secondary btn-sm" onclick={goToday}
				>今日</button
			>
		</div>
		<div class="tl-header-actions">
			{#if view === "plan"}
				<button type="button" class="btn btn-primary" onclick={openNewPlan}>
					<Icon name="add_box" /> 計画追加
				</button>
			{:else}
				<button type="button" class="btn btn-primary" onclick={openNewRecord}>
					<Icon name="edit_note" /> 記録追加
				</button>
			{/if}
		</div>
	</div>

	<div class="tl-list">
		{#if view === "plan"}
			{#each blocks as block (block.id)}
				<PlanCard {block} onedit={onEditPlan} ondelete={onDeletePlan} />
			{:else}
				<div class="tl-empty">計画はまだありません</div>
			{/each}
		{:else}
			{#each records as record (record.id)}
				<RecordCard {record} ondelete={onDeleteRecord} />
			{:else}
				<div class="tl-empty">記録はまだありません</div>
			{/each}
		{/if}
	</div>
</section>

<PlanBlockModal bind:open={planOpen} {editingBlock} onsave={savePlan} />
<RecordModal bind:open={recordOpen} onsave={saveRecord} />

<style>
	/* 前後に計画・記録が無い方向の日付ボタン */
	.btn-icon:disabled {
		color: var(--text-disabled);
		cursor: not-allowed;
		opacity: 0.5;
	}
	.btn-icon:disabled:hover {
		background-color: transparent;
		border-color: var(--border-matte);
	}

	/* 日付ラベルを押すと日付ピッカーが開く（透明な date 入力を重ねる） */
	.tl-date-pick {
		position: relative;
		padding: 4px 8px;
		border-radius: 3px;
		cursor: pointer;
	}
	.tl-date-pick:hover {
		background-color: rgba(127, 127, 127, 0.08);
	}
	.tl-date-pick input {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		opacity: 0;
		cursor: pointer;
	}
</style>
