<script lang="ts">
// Discord 独自Bot トークン（マスク表示）カード（旧 discord-config-form + fetchDiscordSettings）。
// system_default は管理者のみ表示（親のガードで制御）。

import { ApiError } from "$lib/api/client";
import { settingsApi } from "$lib/api/services";
import { pushToast } from "$lib/stores/toast";
import type { DiscordTokenResponse } from "./configTypes";

interface Props {
	/** botId 変更でトークン再取得のトリガに使う */
	botId: string;
	/** 保存後に親のカード群を再描画（招待リンク同期反映など） */
	onsaved?: () => void;
}
let { botId, onsaved }: Props = $props();

let token = $state("");
// 読み込み時（マスク済み）／最後に保存した値。未変更なら保存ボタンを無効にする。
let loadedToken = $state("");
let saving = $state(false);
const canSave = $derived(!saving && token.trim() !== loadedToken);

// botId 変更でマスク済みトークンを再取得（/api/settings/discord は user-scoped）。
$effect(() => {
	void botId;
	void load();
});

async function load() {
	try {
		const data = (await settingsApi.getDiscord()) as DiscordTokenResponse;
		token = data.tokenMasked ?? "";
	} catch {
		token = "";
	}
	loadedToken = token;
}

async function submit(e: SubmitEvent) {
	e.preventDefault();
	if (!canSave) return;
	saving = true;
	try {
		const res = await settingsApi.updateDiscord({ token: token.trim() });
		pushToast(res.message ?? "Discord 設定を保存しました。", "success");
		loadedToken = token.trim();
		onsaved?.();
	} catch (err) {
		pushToast(
			err instanceof ApiError ? err.message : "保存に失敗しました。",
			"error",
		);
	} finally {
		saving = false;
	}
}
</script>

<section class="config-card card">
	<div class="column-header badge-right">
		<h3><span class="material-symbols-outlined header-icon-symbol">robot_2</span>Discord 独自Bot 設定</h3>
		<span class="badge badge-accent">任意設定</span>
	</div>
	<form onsubmit={submit} class="token-form plain-fields">
		<label for="discord-token">Discord Bot Token</label>
		<div class="token-row">
			<input
				type="password"
				id="discord-token"
				placeholder="トークンを変更・登録する場合は入力してください (マスク表示中)"
				autocomplete="new-password"
				bind:value={token}
				disabled={saving}
			/>
			<button
				type="submit"
				class="icon-action"
				disabled={!canSave}
				aria-label="Discord 設定を保存"
				title="Discord 設定を保存"
			>
				<span class="material-symbols-outlined" aria-hidden="true">check</span>
			</button>
		</div>
	</form>
</section>

<style>
	.token-form label {
		font-size: 0.9rem;
		color: var(--text-primary);
	}
	.token-form {
		display: flex;
		flex-direction: column;
		gap: 8px;
		margin-top: 12px;
	}
	.token-row {
		display: flex;
		gap: 12px;
	}
	.token-row input {
		flex: 1;
		min-width: 0;
	}
</style>
