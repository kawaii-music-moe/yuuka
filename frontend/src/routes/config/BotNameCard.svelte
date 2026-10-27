<script lang="ts">
// Bot登録名カード（旧 bot-name-form / fetchBotAttributeConfig の name 部分）。
// owner/Admin かつ非デフォルト Bot のときだけ親が描画する。

import { untrack } from "svelte";
import { ApiError } from "$lib/api/client";
import { botAttributeApi } from "$lib/api/services";
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
let linked = $state(false);
let saving = $state(false);
// 同期解除後の親の再取得でも未保存の入力を維持する。
let lastBotId = "";
let lastSavedName = $state("");
const canSave = $derived(
	!saving && !!name.trim() && name.trim() !== lastSavedName,
);
$effect(() => {
	const current = bot;
	const currentBotId = botId;
	untrack(() => {
		if (lastBotId !== currentBotId || current.name !== lastSavedName) {
			name = current.name ?? "";
		}
		lastBotId = currentBotId;
		lastSavedName = current.name;
		linked = current.name_linked_to_discord ?? false;
	});
});

async function toggleLink() {
	if (saving) return;
	saving = true;
	try {
		const next = !linked;
		const response = await botAttributeApi.updateBotProfile({
			botId,
			nameLinkedToDiscord: next,
		});
		linked = next;
		if (next) name = response.name;
		lastSavedName = response.name;
		pushToast(
			next
				? "Discordアカウント名との同期を有効にしました。"
				: "Discordアカウント名との同期を解除しました。",
			"success",
		);
		onsaved?.();
	} catch (err) {
		pushToast(
			err instanceof ApiError ? err.message : "変更に失敗しました。",
			"error",
		);
	} finally {
		saving = false;
	}
}

async function submit(e: SubmitEvent) {
	e.preventDefault();
	if (!canSave) return;
	const trimmed = name.trim();
	if (!trimmed) {
		pushToast("登録名を入力してください。", "error");
		return;
	}
	saving = true;
	try {
		const response = await botAttributeApi.updateBotProfile({
			botId,
			name: trimmed,
		});
		name = response.name;
		lastSavedName = response.name;
		linked = response.name_linked_to_discord;
		pushToast("登録名を変更しました。", "success");
		onsaved?.();
	} catch (err) {
		pushToast(
			err instanceof ApiError ? err.message : "変更に失敗しました。",
			"error",
		);
	} finally {
		saving = false;
	}
}
</script>

<section class="config-card card">
	<div class="column-header">
		<h3><span class="material-symbols-outlined header-icon-symbol">badge</span>Bot登録名</h3>
	</div>
	<form onsubmit={submit} class="name-form plain-fields">
		<div class="input-wrap">
			<input
				type="text"
				required
				maxlength="50"
				placeholder="例: 経理アシスタント"
				autocomplete="off"
				bind:value={name}
				aria-label="Bot登録名"
				disabled={saving}
			/>
			<button
				type="button"
				class="inline-button link-button"
				class:linked
				aria-label={linked ? "Discordアカウント名との同期を解除" : "Discordアカウント名と同期"}
				title={linked ? "Discordアカウント名との同期を解除" : "Discordアカウント名と同期"}
				aria-pressed={linked}
				disabled={saving}
				onclick={toggleLink}
			><span class="material-symbols-outlined" aria-hidden="true">{linked ? "link" : "link_off"}</span></button>
		</div>
		<button
			type="submit"
			class="icon-action"
			disabled={!canSave}
			aria-label="登録名を保存"
			title="登録名を保存"
		>
			<span class="material-symbols-outlined" aria-hidden="true">check</span>
		</button>
	</form>
</section>

<style>
	.name-form {
		display: flex;
		gap: 12px;
		margin-top: 12px;
	}
	.link-button.linked {
		color: var(--color-primary);
	}
	.link-button:disabled {
		opacity: 0.5;
		cursor: default;
	}
</style>
