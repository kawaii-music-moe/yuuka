<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// スケジュール タブ（旧 app.js fetchSchedulesList / scheduleForm submit /
// handleDeleteSchedule + index.html #tab-schedules / #modal-schedule を移植）。
//
// 挙動の忠実移植:
//   - 表示期間フィルタ（直近7日 / 30日）: ローカル state（旧 data-days）。
//   - 予定追加はモーダル → scheduleApi.add。
//   - 削除は ConfirmDialog（旧 confirm、「Googleカレンダー側からも削除されます」）。
//   - Google 同期は各カードの「Google同期済み」バッジで表示（google_calendar_id 有り時）。
//   - activeBot 変更でリロード（bot-scoped API）。
// ─────────────────────────────────────────────────────────────────────────

import { ApiError } from "$lib/api/client";
import { scheduleApi } from "$lib/api/services";
import type { ScheduleRecord } from "$lib/api/types";
import { Button, confirmDialog, EmptyState } from "$lib/components/ui";
import { activeBot } from "$lib/stores/activeBot";
import { pushToast } from "$lib/stores/toast";

import ScheduleCard from "./schedules/ScheduleCard.svelte";
import ScheduleModal from "./schedules/ScheduleModal.svelte";

let days = $state(7);
let schedules = $state<ScheduleRecord[]>([]);
let loading = $state(false);
let modalOpen = $state(false);

function reportError(e: unknown) {
	const msg = e instanceof ApiError ? e.message : "エラーが発生しました";
	pushToast(msg, "error");
}

// ── 一覧取得（旧 fetchSchedulesList） ──
async function load() {
	loading = true;
	try {
		const res = await scheduleApi.list(days);
		schedules = res.schedules ?? [];
	} catch (e) {
		reportError(e);
		schedules = [];
	} finally {
		loading = false;
	}
}

// activeBot（bot-scoped）変更 or days 変更で再取得。
$effect(() => {
	void $activeBot?.id;
	void days;
	void load();
});

// ── 削除（旧 handleDeleteSchedule、confirm → ConfirmDialog） ──
async function onDelete(id: number) {
	const ok = await confirmDialog({
		message:
			"本当にこの予定を削除しますか？ Googleカレンダー側からも削除されます。",
		danger: true,
		confirmLabel: "削除",
	});
	if (!ok) return;
	try {
		await scheduleApi.delete(id);
		await load();
	} catch (e) {
		reportError(e);
	}
}

// ── 予定追加（旧 scheduleForm submit） ──
async function saveSchedule(payload: {
	title: string;
	description: string;
	startAt: string;
	endAt?: string;
	remindBeforeMinutes: number;
}) {
	try {
		await scheduleApi.add(payload);
		modalOpen = false;
		await load();
	} catch (e) {
		reportError(e);
	}
}
</script>

<section class="tab-view">
	<div class="view-actions-card card">
		<div class="filters-group">
			<span class="label-text">表示期間:</span>
			<button
				type="button"
				class="btn btn-filter"
				class:active={days === 7}
				onclick={() => (days = 7)}>直近7日間</button
			>
			<button
				type="button"
				class="btn btn-filter"
				class:active={days === 30}
				onclick={() => (days = 30)}>直近30日間</button
			>
		</div>
		<Button variant="primary" onclick={() => (modalOpen = true)}
			>＋ 予定追加</Button
		>
	</div>

	<div class="list-container">
		{#if schedules.length > 0}
			{#each schedules as schedule (schedule.id)}
				<ScheduleCard {schedule} {onDelete} />
			{/each}
		{:else if !loading}
			<EmptyState icon="event_busy" message="予定が登録されていません。" />
		{/if}
	</div>
</section>

<ScheduleModal bind:open={modalOpen} onsave={saveSchedule} />
