<script lang="ts">
// Bot登録名カード（旧 bot-name-form / fetchBotAttributeConfig の name 部分）。
// owner/Admin かつ非デフォルト Bot のときだけ親が描画する。

import { ApiError } from "$lib/api/client";
import { botAttributeApi } from "$lib/api/services";
import { Button } from "$lib/components/ui";
import { pushToast } from "$lib/stores/toast";
import type { BotAttrView } from "./configTypes";

interface Props {
	botId: string;
	bot: BotAttrView;
	/** 保存成功後に親へ通知（Bot一覧・サイドバー再取得など） */
	onsaved?: () => void;
}
let { botId, bot, onsaved }: Props = $props();

let name = $state("");
// bot が変わるたびに現在の登録名で初期化。
$effect(() => {
	name = bot.name ?? "";
});

async function submit(e: SubmitEvent) {
	e.preventDefault();
	const trimmed = name.trim();
	if (!trimmed) {
		pushToast("登録名を入力してください。", "error");
		return;
	}
	try {
		await botAttributeApi.updateBotProfile({ botId, name: trimmed });
		pushToast("登録名を変更しました。", "success");
		onsaved?.();
	} catch (err) {
		pushToast(
			err instanceof ApiError ? err.message : "変更に失敗しました。",
			"error",
		);
	}
}
</script>

<details class="config-card card" open>
	<summary class="column-header">
		<h3><span class="material-symbols-outlined header-icon-symbol">badge</span>Bot登録名</h3>
	</summary>
	<p class="description-text">
		このBotの登録名（管理画面・Bot選択画面で表示される名前）を変更します。Discord上の表示名とは別です。
	</p>
	<form onsubmit={submit} class="name-form">
		<input
			type="text"
			required
			maxlength="50"
			placeholder="例: 経理アシスタント"
			autocomplete="off"
			bind:value={name}
		/>
		<Button type="submit" variant="primary">登録名を保存</Button>
	</form>
</details>

<style>
	.name-form {
		display: flex;
		gap: 12px;
		margin-top: 12px;
	}
	.name-form input {
		flex-grow: 1;
	}
</style>
