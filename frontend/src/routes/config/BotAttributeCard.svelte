<script lang="ts">
// Bot属性（プリセット）カード（旧 fetchBotAttributeConfig attr 部分 + btn-save-bot-attribute）。

import { get } from "svelte/store";
import { ApiError } from "$lib/api/client";
import { botAttributeApi } from "$lib/api/services";
import { Button, confirmDialog } from "$lib/components/ui";
import { activeBot, selectBot } from "$lib/stores/activeBot";
import { pushToast } from "$lib/stores/toast";
import type { BotAttrView, PresetItem, PresetsResp } from "./configTypes";

interface Props {
	botId: string;
	bot: BotAttrView;
	/** 変更成功後に親の属性系カードを再取得 */
	onchanged?: () => void;
}
let { botId, bot, onchanged }: Props = $props();

let presets = $state<PresetItem[]>([]);
let selected = $state("");
const currentBadge = $derived(bot.preset_display_name ?? "");

// bot 変更でプリセット一覧を取得し、現在値を選択。
$effect(() => {
	void botId;
	void load(bot.preset ?? "secretary");
});

async function load(current: string) {
	try {
		// presets() の戻り型（types.ts PresetOption）は config タブが読む形状（id/displayName/
		// capabilities）と異なるため unknown 経由でこのタブ用の PresetsResp に読み替える。
		const res = (await botAttributeApi.presets()) as unknown as PresetsResp;
		presets = res.presets ?? [];
	} catch {
		presets = [];
	}
	selected = current;
}

function labelFor(p: PresetItem): string {
	return `${p.displayName}（${p.capabilities.join(" + ")}）`;
}

async function save() {
	if (!selected) return;
	const p = presets.find((x) => x.id === selected);
	const presetLabel = p ? labelFor(p) : selected;
	const ok = await confirmDialog({
		message: `Botの属性を「${presetLabel}」へ変更しますか？\n機能セットが切り替わります（会話の文脈は属性ごとに分離されます）。`,
		confirmLabel: "変更する",
	});
	if (!ok) return;
	try {
		const res = await botAttributeApi.updateAttributes({
			botId,
			preset: selected,
		});
		pushToast(res.message ?? "変更しました。", "success");
		// activeBot の preset も更新（サイドバーのタブフィルタに反映）。
		const cur = get(activeBot);
		if (cur && cur.id === botId) selectBot({ ...cur, preset: selected });
		onchanged?.();
	} catch (err) {
		pushToast(
			err instanceof ApiError ? err.message : "変更に失敗しました。",
			"error",
		);
	}
}
</script>

<details class="config-card card">
	<summary class="column-header badge-right">
		<h3><span class="material-symbols-outlined header-icon-symbol">category</span>Bot属性（プリセット）</h3>
		{#if currentBadge}<span class="badge badge-accent">{currentBadge}</span>{/if}
	</summary>
	<p class="description-text">
		このBotの機能セットを選択します。変更は次のメッセージ処理から即時反映されます。
	</p>
	<div class="attr-row">
		<select bind:value={selected}>
			{#each presets as p (p.id)}
				<option value={p.id}>{labelFor(p)}</option>
			{/each}
		</select>
		<Button variant="primary" onclick={save}>属性を変更</Button>
	</div>
	<span class="field-sub">
		※「汎用モード」はサーバー常駐の簡易Bot（MCP接続 + ペルソナ + メモリ）です。タスク・家計などの秘書機能は無効になります。
	</span>
</details>

<style>
	.attr-row {
		display: flex;
		gap: 12px;
		margin-top: 12px;
	}
	.attr-row select {
		flex-grow: 1;
	}
</style>
