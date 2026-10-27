<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// リマインダー タブ（旧 app.js fetchRemindersList / reminder-form + index.html
// #tab-reminders を移植）。reminderApi 使用（bot-scoped）。
//
// 挙動の忠実移植:
//   - 登録フォーム（メッセージ / 通知日時 / cron / 送信先 / チャンネルID）。
//   - 一覧（全件表示チェックボックスで sent/cancelled も含める → ?all=1）。
//   - pending のみキャンセル可（ConfirmDialog で確認）。
//   - activeBot 変更でリロード。
// ─────────────────────────────────────────────────────────────────────────

import { ApiError } from "$lib/api/client";
import { reminderApi, settingsApi } from "$lib/api/services";
import type { ReminderRecord } from "$lib/api/types";
import { confirmDialog, EmptyState } from "$lib/components/ui";
import { activeBot } from "$lib/stores/activeBot";
import { pushToast } from "$lib/stores/toast";
import type { StatusConfigResponse } from "./config/configTypes";
import CronField from "./delivery/CronField.svelte";
import TargetField from "./delivery/TargetField.svelte";

import ReminderCard from "./reminders/ReminderCard.svelte";
import { toTriggerAt } from "./reminders/reminderUtils";

let reminders = $state<ReminderRecord[]>([]);
let showAll = $state(false);
let loading = $state(false);

// フォーム state
let message = $state("");
let triggerAt = $state("");
let repeatRule = $state("");
let targetType = $state<"dm" | "channel">("dm");
let targetId = $state("");
// ユーザー設定の既定送信先。送信先の初期値にし、選択肢に（既定）を付ける。
let defaultTarget = $state<{ type: "dm" | "channel"; id: string } | null>(null);
let submitting = $state(false);

const targetOptions = $derived<{ value: "dm" | "channel"; label: string }[]>([
	{ value: "dm", label: `DM${defaultTarget?.type === "dm" ? "（既定）" : ""}` },
	{
		value: "channel",
		label: `チャンネル${defaultTarget?.type === "channel" ? "（既定）" : ""}`,
	},
]);

function resetTarget() {
	targetType = defaultTarget?.type ?? "dm";
	targetId = defaultTarget?.type === "channel" ? defaultTarget.id : "";
}

// 通知日時は欄のどこを押してもブラウザ標準の日時ピッカーを開く（未対応ブラウザでは何もしない）。
function openPicker(e: MouseEvent) {
	const input = e.currentTarget as HTMLInputElement;
	try {
		input.showPicker?.();
	} catch {
		// ユーザー操作扱いにならない等で開けないときは、通常の入力欄のままにする
	}
}

function reportError(e: unknown) {
	const msg = e instanceof ApiError ? e.message : "エラーが発生しました";
	pushToast(msg, "error");
}

async function loadReminders() {
	loading = true;
	try {
		const res = await reminderApi.list({ all: showAll });
		reminders = res.reminders ?? [];
	} catch (e) {
		reportError(e);
		reminders = [];
	} finally {
		loading = false;
	}
}

async function loadDefaultTarget() {
	defaultTarget = null;
	try {
		const res = (await settingsApi.status()) as StatusConfigResponse;
		const t = res.config?.notifyTarget;
		defaultTarget =
			t?.type === "channel"
				? { type: "channel", id: t.id ?? "" }
				: { type: "dm", id: "" };
	} catch {
		defaultTarget = null;
	}
	resetTarget();
}

// activeBot（bot-scoped）変更 or showAll 変更で再取得。
$effect(() => {
	void $activeBot?.id;
	void showAll;
	void loadReminders();
});

$effect(() => {
	void $activeBot?.id;
	void loadDefaultTarget();
});

async function onSubmit(e: SubmitEvent) {
	e.preventDefault();
	const msg = message.trim();
	if (!msg || !triggerAt) return;
	submitting = true;
	try {
		await reminderApi.add({
			message: msg,
			trigger_at: toTriggerAt(triggerAt),
			...(repeatRule.trim() ? { repeat_rule: repeatRule.trim() } : {}),
			target_type: targetType,
			...(targetType === "channel" && targetId.trim()
				? { target_id: targetId.trim() }
				: {}),
		});
		// フォームリセット（旧 reminderForm.reset()）
		message = "";
		triggerAt = "";
		repeatRule = "";
		resetTarget();
		pushToast("リマインダーを登録しました。", "success");
		await loadReminders();
	} catch (err) {
		reportError(err);
	} finally {
		submitting = false;
	}
}

async function onCancel(id: number) {
	const target = reminders.find((r) => r.id === id);
	const ok = await confirmDialog({
		message: `リマインダー「${target?.message ?? ""}」をキャンセルしますか？`,
		danger: true,
		confirmLabel: "キャンセルする",
	});
	if (!ok) return;
	try {
		await reminderApi.cancel(id);
		await loadReminders();
	} catch (e) {
		reportError(e);
	}
}
</script>

<section class="tab-view">
	<div class="expense-actions-columns">
		<!-- 登録フォーム -->
		<div class="action-column card">
			<div class="column-header">
				<h3>
					<span class="material-symbols-outlined header-icon-symbol">alarm_add</span
					>リマインダーの登録
				</h3>
			</div>
			<form class="form-stack" onsubmit={onSubmit}>
				<div class="form-group">
					<label for="reminder-message">メッセージ *</label>
					<input
						type="text"
						id="reminder-message"
						required
						placeholder="例: ゴミ出しの時間です"
						bind:value={message}
					/>
				</div>
				<div class="form-group">
					<label for="reminder-trigger-at">通知日時 *</label>
					<input
						type="datetime-local"
						id="reminder-trigger-at"
						required
						class="picker-input"
						onclick={openPicker}
						bind:value={triggerAt}
					/>
				</div>
				<div class="form-group">
					<CronField
						id="reminder-repeat-rule"
						label="繰り返し（任意）"
						placeholder="例: 0 9 * * 1 (毎週月曜9時)"
						help="指定すると送信後も自動で次回が予約されます。"
						bind:value={repeatRule}
					/>
				</div>
				<div class="form-group">
					<label for="reminder-target-type">送信先</label>
					<TargetField
						id="reminder-target-type"
						idInputId="reminder-target-id"
						options={targetOptions}
						bind:type={targetType}
						bind:targetId
					/>
				</div>
				<button type="submit" class="btn btn-primary btn-block" disabled={submitting}
					>リマインダーを登録</button
				>
			</form>
		</div>

		<!-- 一覧 -->
		<div class="action-column card">
			<div class="column-header action-right">
				<h3>
					<span class="material-symbols-outlined header-icon-symbol"
						>notifications</span
					>リマインダー一覧
				</h3>
				<div class="reminders-showall-group">
					<label for="reminders-show-all">全件表示</label>
					<input type="checkbox" id="reminders-show-all" bind:checked={showAll} />
				</div>
			</div>
			<div class="reminders-list">
				{#if reminders.length > 0}
					{#each reminders as rem (rem.id)}
						<ReminderCard reminder={rem} oncancel={onCancel} />
					{/each}
				{:else if !loading}
					<EmptyState
						icon="notifications_off"
						message="リマインダーは登録されていません。"
					/>
				{/if}
			</div>
		</div>
	</div>
</section>

<style>
	.picker-input {
		cursor: pointer;
	}
	/* 中身が長くても段の幅を広げない（grid の 1fr は既定で中身の幅より縮まない） */
	.action-column {
		min-width: 0;
	}
	/* 見出しの並びの右端に置く */
	.reminders-showall-group {
		display: flex;
		align-items: center;
		gap: 8px;
		margin-left: auto;
	}
	.reminders-showall-group label {
		margin: 0;
		font-size: 0.8rem;
	}
	.reminders-showall-group input {
		margin: 0;
	}
	.reminders-list {
		display: flex;
		flex-direction: column;
		gap: 10px;
		max-height: 560px;
		overflow-x: hidden;
		overflow-y: auto;
		/* .card-item は左右に 12px はみ出す（margin: 0 -12px）ので、その分を枠に含めて横スクロールを出さない */
		margin: 12px -12px 0;
		padding: 0 12px;
	}
</style>
