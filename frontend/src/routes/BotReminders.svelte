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
import { reminderApi } from "$lib/api/services";
import type { ReminderRecord } from "$lib/api/types";
import { confirmDialog, EmptyState } from "$lib/components/ui";
import { activeBot } from "$lib/stores/activeBot";
import { pushToast } from "$lib/stores/toast";

import ReminderCard from "./reminders/ReminderCard.svelte";
import { toTriggerAt } from "./reminders/reminderUtils";

let reminders = $state<ReminderRecord[]>([]);
let showAll = $state(false);
let loading = $state(false);

// フォーム state
let message = $state("");
let triggerAt = $state("");
let repeatRule = $state("");
let targetType = $state<"" | "dm" | "channel">("");
let targetId = $state("");
let submitting = $state(false);

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

// activeBot（bot-scoped）変更 or showAll 変更で再取得。
$effect(() => {
	void $activeBot?.id;
	void showAll;
	void loadReminders();
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
			...(targetType ? { target_type: targetType } : {}),
			...(targetId.trim() ? { target_id: targetId.trim() } : {}),
		});
		// フォームリセット（旧 reminderForm.reset()）
		message = "";
		triggerAt = "";
		repeatRule = "";
		targetType = "";
		targetId = "";
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
			<p class="description-text">
				指定日時にDiscordへ通知するリマインダーを登録します。cron式を指定すると繰り返しリマインドになります。
			</p>
			<form class="reminder-form" onsubmit={onSubmit}>
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
						bind:value={triggerAt}
					/>
				</div>
				<div class="form-group">
					<label for="reminder-repeat-rule">繰り返し (cron式・任意)</label>
					<input
						type="text"
						id="reminder-repeat-rule"
						placeholder="例: 0 9 * * 1 (毎週月曜9時)"
						class="reminder-cron-input"
						bind:value={repeatRule}
					/>
					<span class="field-sub">※指定すると送信後も自動で次回が予約されます。</span>
				</div>
				<div class="form-row">
					<div class="form-group">
						<label for="reminder-target-type">送信先</label>
						<select id="reminder-target-type" bind:value={targetType}>
							<option value="">既定 (ユーザー設定に従う)</option>
							<option value="dm">DM</option>
							<option value="channel">チャンネル</option>
						</select>
					</div>
					<div class="form-group">
						<label for="reminder-target-id">チャンネルID (チャンネル選択時)</label>
						<input
							type="text"
							id="reminder-target-id"
							placeholder="例: 123456789012345678"
							bind:value={targetId}
						/>
					</div>
				</div>
				<button type="submit" class="btn btn-primary btn-block" disabled={submitting}
					>リマインダーを登録</button
				>
			</form>
		</div>

		<!-- 一覧 -->
		<div class="action-column card">
			<div class="column-header">
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
	.reminder-form {
		display: flex;
		flex-direction: column;
		gap: 12px;
		margin-top: 8px;
	}
	.reminder-cron-input {
		font-family: var(--font-family-mono);
	}
	.reminders-showall-group {
		display: flex;
		align-items: center;
		gap: 8px;
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
		overflow-y: auto;
		padding-right: 4px;
		margin-top: 12px;
	}
</style>
