<script lang="ts">
// チェック状態でパーソナル秘書 / 汎用モードを選択する。

import { get } from "svelte/store";
import { ApiError } from "$lib/api/client";
import { botAttributeApi } from "$lib/api/services";
import { Checkbox } from "$lib/components/ui";
import { activeBot, selectBot } from "$lib/stores/activeBot";
import { pushToast } from "$lib/stores/toast";
import type { BotAttrView } from "./configTypes";

interface Props {
	botId: string;
	bot: BotAttrView;
	/** 変更成功後に親の属性系カードを再取得 */
	onchanged?: () => void;
}
let { botId, bot, onchanged }: Props = $props();

let secretaryEnabled = $state(false);
let saving = $state(false);
const currentPreset = $derived(bot.preset ?? "secretary");

// Bot 切替・保存後の再取得時に現在のモードを反映。
$effect(() => {
	void botId;
	secretaryEnabled = currentPreset === "secretary";
});

async function save(enabled: boolean) {
	if (saving) return;
	const preset = enabled ? "secretary" : "mcp_assistant";
	if (preset === currentPreset) return;
	const id = botId;
	const previousPreset = currentPreset;
	saving = true;
	try {
		const res = await botAttributeApi.updateAttributes({
			botId: id,
			preset,
		});
		pushToast(res.message ?? "変更しました。", "success");
		// activeBot の preset も更新（サイドバーのタブフィルタに反映）。
		const cur = get(activeBot);
		if (cur && cur.id === id) selectBot({ ...cur, preset });
		onchanged?.();
	} catch (err) {
		if (botId === id) secretaryEnabled = previousPreset === "secretary";
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
		<h3><span class="material-symbols-outlined header-icon-symbol">category</span>Botモード</h3>
	</div>
	<div class="attr-row">
		<Checkbox
			bind:checked={secretaryEnabled}
			disabled={saving}
			label="Enable secretary mode"
			onchange={save}
		/>
		{#if saving}<span role="status">変更中…</span>{/if}
	</div>
</section>

<style>
	.attr-row {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 12px;
		margin-top: 12px;
	}
</style>
