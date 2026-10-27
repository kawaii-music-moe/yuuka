<script lang="ts">
// 時刻と通知（ユーザー個別・全Bot共通）カード（旧 user-settings-form + fetchConfigSettings の user 部分）。
// 「あなたの設定」は Bot を選ばずに開くため、初期値は bot-scoped の /api/status ではなく
// GET /api/settings/user から読む。保存は POST /api/settings/user。

import { onMount } from "svelte";
import { ApiError } from "$lib/api/client";
import { settingsApi } from "$lib/api/services";
import { Button } from "$lib/components/ui";
import { pushToast } from "$lib/stores/toast";

let loaded = $state(false);
let richReply = $state(true);
let remindDefault = $state(10);
let notifyType = $state<"dm" | "channel">("dm");
let notifyId = $state("");
let timezone = $state("");

onMount(async () => {
	try {
		const c = await settingsApi.getUserSettings();
		richReply = c.richReplyEnabled !== false;
		remindDefault = c.remindDefaultMinutes ?? 10;
		notifyType = c.notifyTarget?.type === "channel" ? "channel" : "dm";
		notifyId = c.notifyTarget?.id ?? "";
		timezone = c.timezone ?? "";
		loaded = true;
	} catch (err) {
		pushToast(
			err instanceof ApiError ? err.message : "設定を読み込めませんでした。",
			"error",
		);
	}
});

async function submit(e: SubmitEvent) {
	e.preventDefault();
	const payload: Record<string, unknown> = {
		richReplyEnabled: richReply,
		remindDefaultMinutes: Number(remindDefault) || 0,
		notifyTargetType: notifyType,
		notifyTargetId: notifyId.trim(),
	};
	if (timezone.trim()) payload.timezone = timezone.trim();
	try {
		const res = await settingsApi.updateUserSettings(payload);
		pushToast(
			res.message ?? "時刻と通知の設定を保存しました。",
			res.success ? "success" : "error",
		);
	} catch (err) {
		pushToast(
			err instanceof ApiError ? err.message : "設定の保存に失敗しました。",
			"error",
		);
	}
}
</script>

<section class="config-card card">
	<div class="column-header">
		<h3><span class="material-symbols-outlined header-icon-symbol">tune</span>時刻と通知</h3>
	</div>
	<p class="description-text">あなたのすべてのBotに共通の設定です。</p>
	<form onsubmit={submit} class="user-form">
		<div class="form-row">
			<div class="form-group">
				<label for="user-timezone">タイムゾーン</label>
				<input type="text" id="user-timezone" placeholder="例: Asia/Tokyo" bind:value={timezone} />
			</div>
			<div class="form-group">
				<label for="user-remind-default">リマインド既定 (分前)</label>
				<input type="number" id="user-remind-default" min="0" placeholder="10" bind:value={remindDefault} />
			</div>
		</div>
		<div class="form-row">
			<div class="form-group">
				<label for="user-notify-type">通知先</label>
				<select id="user-notify-type" bind:value={notifyType}>
					<option value="dm">DM</option>
					<option value="channel">チャンネル</option>
				</select>
			</div>
			{#if notifyType === "channel"}
				<div class="form-group">
					<label for="user-notify-id">通知先チャンネルID</label>
					<input type="text" id="user-notify-id" placeholder="例: 123456789012345678" bind:value={notifyId} />
				</div>
			{/if}
		</div>
		<div class="form-group checkbox-inline">
			<input type="checkbox" id="user-rich-reply" bind:checked={richReply} />
			<label for="user-rich-reply">リッチ返信 (Embed表示) を有効にする</label>
		</div>
		<Button type="submit" variant="primary" disabled={!loaded}>保存</Button>
	</form>
</section>

<style>
	.user-form {
		display: flex;
		flex-direction: column;
		gap: 12px;
		margin-top: 16px;
	}
</style>
