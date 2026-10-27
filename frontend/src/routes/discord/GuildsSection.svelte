<script lang="ts">
// 応答許可ギルド（旧 renderAssistantGuilds + btn-add-assistant-guild）。

import { ApiError } from "$lib/api/client";
import { botAttributeApi } from "$lib/api/services";
import { confirmDialog, Icon } from "$lib/components/ui";
import { pushToast } from "$lib/stores/toast";
import type { AssistantGuild } from "../config/configTypes";
import AddButton from "./AddButton.svelte";
import DeleteButton from "./DeleteButton.svelte";

interface Props {
	botId: string;
	guilds: AssistantGuild[];
	/** 追加/削除の成功後に親へ再取得を要求 */
	onchanged: () => void;
}
let { botId, guilds, onchanged }: Props = $props();

let guildInput = $state("");

async function paste() {
	try {
		guildInput = (await navigator.clipboard.readText()).trim();
	} catch {
		pushToast("クリップボードを読み取れませんでした。", "error");
	}
}

async function add() {
	const guildId = guildInput.trim();
	if (!/^\d{5,25}$/.test(guildId)) {
		pushToast(
			"ギルドID（数字）を入力してください。Discordの開発者モードでサーバーを右クリック →「IDをコピー」で取得できます。",
			"error",
		);
		return;
	}
	try {
		const res = await botAttributeApi.setGuilds({
			botId,
			guildId,
			action: "add",
		});
		if (res.success) {
			guildInput = "";
			onchanged();
		} else pushToast(res.message ?? "操作に失敗しました。", "error");
	} catch (err) {
		pushToast(
			err instanceof ApiError ? err.message : "通信エラーが発生しました。",
			"error",
		);
	}
}

async function remove(g: AssistantGuild) {
	const label = g.guild_name
		? `${g.guild_name}（${g.guild_id}）`
		: `ギルド ${g.guild_id}`;
	const ok = await confirmDialog({
		message: `${label} を応答許可リストから削除しますか？`,
		danger: true,
		confirmLabel: "削除",
	});
	if (!ok) return;
	try {
		const res = await botAttributeApi.setGuilds({
			botId,
			guildId: g.guild_id,
			action: "remove",
		});
		if (res.success) onchanged();
		else pushToast(res.message ?? "操作に失敗しました。", "error");
	} catch (err) {
		pushToast(
			err instanceof ApiError ? err.message : "通信エラーが発生しました。",
			"error",
		);
	}
}
</script>

<section class="config-card card">
	<div class="column-header">
		<h3><span class="material-symbols-outlined header-icon-symbol">dns</span>応答許可ギルド</h3>
	</div>
	<div class="section-body">
		<div class="add-row no-wrap">
			<div class="input-wrap">
				<input type="text" placeholder="ギルドID（数字）" class="mono" bind:value={guildInput} />
				<button
					type="button"
					class="inline-button"
					title="クリップボードから貼り付け"
					aria-label="クリップボードから貼り付け"
					onclick={paste}
				><Icon name="content_paste" /></button>
			</div>
			<AddButton onclick={add} disabled={!guildInput.trim()} icon="domain_add" title="ギルドを追加" />
		</div>
		<div class="row-list">
			{#if guilds.length === 0}
				<span class="field-sub">許可ギルドが未登録です。</span>
			{:else}
				{#each guilds as g (g.guild_id)}
					<div class="list-row">
						<span class="row-label">
							{#if g.guild_name}{g.guild_name}（<span class="mono">{g.guild_id}</span>）{:else}<span
									class="mono">{g.guild_id}</span
								>{/if}
						</span>
						<DeleteButton onclick={() => remove(g)} />
					</div>
				{/each}
			{/if}
		</div>
	</div>
</section>

<style>
	/* 共通の .discord-tab .add-row は折り返すが、ギルド行は 1 行に収める */
	.add-row.no-wrap {
		flex-wrap: nowrap;
	}
</style>
