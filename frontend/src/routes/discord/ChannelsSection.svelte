<script lang="ts">
	// 有効化チャンネル（メンション不要で応答する・Rust 新機能）。
	// 応答許可ギルドを選び、そのギルド内のチャンネルID（数字）を登録する。登録チャンネルでは
	// Bot がメンション/返信なしで全メッセージに応答する（利用資格・レート制限は従来どおり適用）。
	import { botAttributeApi } from "$lib/api/services";
	import { ApiError } from "$lib/api/client";
	import { pushToast } from "$lib/stores/toast";
	import { confirmDialog, Button } from "$lib/components/ui";
	import { guildLabel } from "../config/configTypes";
	import type { AssistantGuild, AssistantChannel } from "../config/configTypes";

	interface Props {
		botId: string;
		guilds: AssistantGuild[];
		channels: AssistantChannel[];
		/** 追加/削除の成功後に親へ再取得を要求 */
		onchanged: () => void;
	}
	let { botId, guilds, channels, onchanged }: Props = $props();

	let selectedGuild = $state("");
	let channelInput = $state("");

	// ギルド一覧が変わったら選択を先頭へ補正（RolesSection と同じ挙動）。
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
			const res = await botAttributeApi.setChannels({ botId, guildId, channelId, action: "add" });
			if (res.success) {
				channelInput = "";
				onchanged();
			} else pushToast(res.message ?? "操作に失敗しました。", "error");
		} catch (err) {
			pushToast(err instanceof ApiError ? err.message : "通信エラーが発生しました。", "error");
		}
	}

	async function remove(ch: AssistantChannel, shift: boolean) {
		if (!shift) {
			const label = ch.channel_name ? `#${ch.channel_name}` : `チャンネル ${ch.channel_id}`;
			const ok = await confirmDialog({
				message: `${label} を有効化リストから削除しますか？`,
				danger: true,
				confirmLabel: "削除",
			});
			if (!ok) return;
		}
		try {
			const res = await botAttributeApi.setChannels({
				botId,
				guildId: ch.guild_id,
				channelId: ch.channel_id,
				action: "remove",
			});
			if (res.success) onchanged();
			else pushToast(res.message ?? "操作に失敗しました。", "error");
		} catch (err) {
			pushToast(err instanceof ApiError ? err.message : "通信エラーが発生しました。", "error");
		}
	}
</script>

<details class="form-group collapsible-group">
	<summary>有効化チャンネル（メンション不要）</summary>
	<p class="description-text sub">
		登録したチャンネルでは、Botへのメンションや返信が無くても全メッセージに応答します。雑談チャンネルなどで気軽に会話させたい場合に使います。利用資格（メンバー/ロール）・レート制限は通常どおり適用されます。削除ボタンはShiftを押しながらで確認なしに削除できます。
	</p>
	<div class="add-row">
		<select bind:value={selectedGuild} class="min180">
			{#each guilds as g (g.guild_id)}
				<option value={g.guild_id}>{g.guild_name ?? `ギルド ${g.guild_id}`}</option>
			{/each}
		</select>
		<input type="text" placeholder="チャンネルID（数字）" class="mono grow" bind:value={channelInput} />
		<Button variant="primary" onclick={add}>追加</Button>
	</div>
	<div class="row-list">
		{#if channels.length === 0}
			<span class="field-sub">有効化チャンネルはありません（全チャンネルでメンション必須）。</span>
		{:else}
			{#each channels as ch (ch.guild_id + ":" + ch.channel_id)}
				<div class="list-row">
					<span class="field-sub">
						{#if ch.channel_name}#{ch.channel_name}（<span class="mono">{ch.channel_id}</span
							>）{:else}<span class="mono">#{ch.channel_id}</span>{/if}
						@ {guildLabel(guilds, ch.guild_id)}
					</span>
					<button
						type="button"
						class="btn btn-secondary btn-sm"
						title="Shiftを押しながらで確認なしに削除"
						onclick={(e) => remove(ch, e.shiftKey)}
					>削除</button>
				</div>
			{/each}
		{/if}
	</div>
</details>

<style>
	.sub {
		margin: 4px 0 8px;
	}
	.add-row {
		display: flex;
		gap: 12px;
		flex-wrap: wrap;
	}
	.min180 {
		min-width: 180px;
	}
	.grow {
		flex-grow: 1;
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
