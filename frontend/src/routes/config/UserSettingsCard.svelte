<script lang="ts">
// アシスタント設定（ユーザー個別）カード（旧 user-settings-form + fetchConfigSettings の user 部分）。
// 初期値は /api/status の config（親から渡す）。保存は /api/settings/user。

import { ApiError } from "$lib/api/client";
import { settingsApi } from "$lib/api/services";
import { Button } from "$lib/components/ui";
import { pushToast } from "$lib/stores/toast";
import type { StatusConfig } from "./configTypes";

interface Props {
	config: StatusConfig | null;
}
let { config }: Props = $props();

let richReply = $state(true);
let remindDefault = $state(10);
let notifyType = $state<"dm" | "channel">("dm");
let notifyId = $state("");
let timezone = $state("");

// config が届いたら初期化（旧 fetchConfigSettings の value 代入）。
$effect(() => {
	const c = config;
	if (!c) return;
	richReply = c.richReplyEnabled !== false;
	remindDefault = c.remindDefaultMinutes ?? 10;
	notifyType = c.notifyTarget?.type === "channel" ? "channel" : "dm";
	notifyId = c.notifyTarget?.id ?? "";
	timezone = c.timezone ?? "";
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
			res.message ?? "アシスタント設定を保存しました。",
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

<details class="config-card card">
	<summary class="column-header">
		<h3><span class="material-symbols-outlined header-icon-symbol">tune</span>アシスタント設定</h3>
	</summary>
	<p class="description-text">
		Discord上でのアシスタントの応答スタイルや通知先の既定値を設定します。
	</p>
	<form onsubmit={submit} class="user-form">
		<div class="form-group checkbox-inline">
			<input type="checkbox" id="user-rich-reply" bind:checked={richReply} />
			<label for="user-rich-reply">リッチ返信 (Embed表示) を有効にする</label>
		</div>
		<div class="form-row">
			<div class="form-group">
				<label for="user-remind-default">リマインド既定 (分前)</label>
				<input type="number" id="user-remind-default" min="0" placeholder="10" bind:value={remindDefault} />
			</div>
			<div class="form-group">
				<label for="user-timezone">タイムゾーン</label>
				<input type="text" id="user-timezone" placeholder="例: Asia/Tokyo" bind:value={timezone} />
			</div>
		</div>
		<div class="form-row">
			<div class="form-group">
				<label for="user-notify-type">通知先タイプ</label>
				<select id="user-notify-type" bind:value={notifyType}>
					<option value="dm">DM</option>
					<option value="channel">チャンネル</option>
				</select>
			</div>
			<div class="form-group">
				<label for="user-notify-id">通知先チャンネルID (チャンネル選択時)</label>
				<input type="text" id="user-notify-id" placeholder="例: 123456789012345678" bind:value={notifyId} />
			</div>
		</div>
		<Button type="submit" variant="primary">アシスタント設定を保存</Button>
	</form>
</details>

<style>
	.user-form {
		display: flex;
		flex-direction: column;
		gap: 12px;
		margin-top: 16px;
	}
	.checkbox-inline {
		display: flex;
		align-items: center;
		gap: 10px;
	}
	.checkbox-inline input {
		width: 20px;
		height: 20px;
	}
</style>
