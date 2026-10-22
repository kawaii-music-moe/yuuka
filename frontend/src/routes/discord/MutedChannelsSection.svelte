<script lang="ts">
// 発言禁止チャンネル（メンション/返信があっても応答しない・Rust 新機能）。
// 有効化チャンネル（ChannelsSection）の逆で、登録チャンネルでは Bot がメンションされても一切応答しない。
// 応答許可ギルドを選び、そのギルド内のチャンネルID（数字）を登録する。

import { ApiError } from "$lib/api/client";
import { botAttributeApi } from "$lib/api/services";
import { confirmDialog } from "$lib/components/ui";
import { pushToast } from "$lib/stores/toast";
import type { AssistantChannel, AssistantGuild } from "../config/configTypes";
import { guildLabel } from "../config/configTypes";
import AddButton from "./AddButton.svelte";
import ChannelInput from "./ChannelInput.svelte";
import DeleteButton from "./DeleteButton.svelte";

interface Props {
	botId: string;
	guilds: AssistantGuild[];
	mutedChannels: AssistantChannel[];
	/** 追加/削除の成功後に親へ再取得を要求 */
	onchanged: () => void;
}
let { botId, guilds, mutedChannels, onchanged }: Props = $props();

let selectedGuild = $state("");
let channelInput = $state("");

// ギルド一覧が変わったら選択を先頭へ補正（ChannelsSection と同じ挙動）。
$effect(() => {
	if (guilds.length === 0) {
		selectedGuild = "";
		return;
	}
	if (!guilds.some((g) => g.guild_id === selectedGuild)) {
		selectedGuild = guilds[0].guild_id;
	}
});

async function add() {
	const guildId = selectedGuild;
	const channelId = channelInput.trim();
	if (!guildId) {
		pushToast("先に応答許可ギルドを追加してください。", "error");
		return;
	}
	if (!/^\d{5,25}$/.test(channelId)) {
		pushToast(
			"チャンネルID（数字）を入力してください。Discordの開発者モードでチャンネルを右クリック →「IDをコピー」で取得できます。",
			"error",
		);
		return;
	}
	try {
		const res = await botAttributeApi.setMutedChannels({
			botId,
			guildId,
			channelId,
			action: "add",
		});
		if (res.success) {
			channelInput = "";
			onchanged();
		} else pushToast(res.message ?? "操作に失敗しました。", "error");
	} catch (err) {
		pushToast(
			err instanceof ApiError ? err.message : "通信エラーが発生しました。",
			"error",
		);
	}
}

async function remove(ch: AssistantChannel, shift: boolean) {
	if (!shift) {
		const label = ch.channel_name
			? `#${ch.channel_name}`
			: `チャンネル ${ch.channel_id}`;
		const ok = await confirmDialog({
			message: `${label} を発言禁止リストから削除しますか？`,
			danger: true,
			confirmLabel: "削除",
		});
		if (!ok) return;
	}
	try {
		const res = await botAttributeApi.setMutedChannels({
			botId,
			guildId: ch.guild_id,
			channelId: ch.channel_id,
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
		<h3><span class="material-symbols-outlined header-icon-symbol">volume_off</span>発言禁止チャンネル</h3>
	</div>
	<div class="section-body">
		<div class="add-row">
			<div class="joined-field">
				<select bind:value={selectedGuild} class="guild-select" aria-label="ギルド">
					{#each guilds as g (g.guild_id)}
						<option value={g.guild_id}>{g.guild_name ?? `ギルド ${g.guild_id}`}</option>
					{/each}
				</select>
				<span class="divider" aria-hidden="true"></span>
				<ChannelInput
					{botId}
					guildId={selectedGuild}
					bind:value={channelInput}
					exclude={mutedChannels.filter((ch) => ch.guild_id === selectedGuild).map((ch) => ch.channel_id)}
				/>
			</div>
			<AddButton onclick={add} disabled={!channelInput.trim()} icon="add_comment" title="チャンネルを追加" />
		</div>
		<div class="row-list">
			{#if mutedChannels.length === 0}
				<span class="field-sub">発言禁止チャンネルはありません。</span>
			{:else}
				{#each mutedChannels as ch (ch.guild_id + ":" + ch.channel_id)}
					<div class="list-row">
						<span class="row-label">
							{#if ch.channel_name}#{ch.channel_name}（<span class="mono">{ch.channel_id}</span
								>）{:else}<span class="mono">#{ch.channel_id}</span>{/if}
							@ {guildLabel(guilds, ch.guild_id)}
						</span>
						<DeleteButton title="Shiftを押しながらで確認なしに削除" onclick={(e) => remove(ch, e.shiftKey)} />
					</div>
				{/each}
			{/if}
		</div>
	</div>
</section>

<style>
	.section-body {
		display: flex;
		flex-direction: column;
		gap: 12px;
		margin-top: 12px;
	}
	.add-row {
		display: flex;
		gap: 12px;
		flex-wrap: wrap;
	}
	.mono {
		font-family: var(--font-family-mono);
	}
	.row-list {
		display: flex;
		flex-direction: column;
		gap: 8px;
		margin-top: 8px;
	}
	.list-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 10px;
	}
</style>
