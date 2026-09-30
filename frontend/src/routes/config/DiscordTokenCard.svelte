<script lang="ts">
// Discord 独自Bot トークン（マスク表示）カード（旧 discord-config-form + fetchDiscordSettings）。
// system_default は管理者のみ表示（親のガードで制御）。

import { ApiError } from "$lib/api/client";
import { settingsApi } from "$lib/api/services";
import { Button } from "$lib/components/ui";
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
}

async function submit(e: SubmitEvent) {
	e.preventDefault();
	try {
		const res = await settingsApi.updateDiscord({ token: token.trim() });
		pushToast(res.message ?? "Discord 設定を保存しました。", "success");
		onsaved?.();
	} catch (err) {
		pushToast(
			err instanceof ApiError ? err.message : "保存に失敗しました。",
			"error",
		);
	}
}
</script>

<details class="config-card card">
	<summary class="column-header badge-right">
		<h3><span class="material-symbols-outlined header-icon-symbol">robot_2</span>Discord 独自Bot 設定</h3>
		<span class="badge badge-accent">任意設定</span>
	</summary>
	<p class="description-text">
		独自の Discord Bot Token を設定できます。未設定の場合はシステムデフォルトのボットが適用されます。
	</p>
	<form onsubmit={submit} class="token-form">
		<div class="form-group">
			<label for="discord-token">Discord Bot Token (任意)</label>
			<input
				type="password"
				id="discord-token"
				placeholder="トークンを変更・登録する場合は入力してください (マスク表示中)"
				autocomplete="new-password"
				bind:value={token}
			/>
			<span class="field-sub">※独自Botを使用しない場合は空欄にしてください。</span>
		</div>
		<Button type="submit" variant="primary">Discord 設定を保存</Button>
	</form>
</details>

<style>
	.token-form {
		display: flex;
		flex-direction: column;
		gap: 12px;
		margin-top: 16px;
	}
</style>
