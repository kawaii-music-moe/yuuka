<script lang="ts">
	// 応答許可ギルド（旧 renderAssistantGuilds + btn-add-assistant-guild）。
	import { botAttributeApi } from "$lib/api/services";
	import { ApiError } from "$lib/api/client";
	import { pushToast } from "$lib/stores/toast";
	import { confirmDialog, Button } from "$lib/components/ui";
	import type { AssistantGuild } from "../config/configTypes";

	interface Props {
		botId: string;
		guilds: AssistantGuild[];
		/** 追加/削除の成功後に親へ再取得を要求 */
		onchanged: () => void;
	}
	let { botId, guilds, onchanged }: Props = $props();

	let guildInput = $state("");

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
			const res = await botAttributeApi.setGuilds({ botId, guildId, action: "add" });
			if (res.success) {
				guildInput = "";
				onchanged();
			} else pushToast(res.message ?? "操作に失敗しました。", "error");
		} catch (err) {
			pushToast(err instanceof ApiError ? err.message : "通信エラーが発生しました。", "error");
		}
	}

	async function remove(g: AssistantGuild) {
		const label = g.guild_name ? `${g.guild_name}（${g.guild_id}）` : `ギルド ${g.guild_id}`;
		const ok = await confirmDialog({
			message: `${label} を応答許可リストから削除しますか？`,
			danger: true,
			confirmLabel: "削除",
		});
		if (!ok) return;
		try {
			const res = await botAttributeApi.setGuilds({ botId, guildId: g.guild_id, action: "remove" });
			if (res.success) onchanged();
			else pushToast(res.message ?? "操作に失敗しました。", "error");
		} catch (err) {
			pushToast(err instanceof ApiError ? err.message : "通信エラーが発生しました。", "error");
		}
	}
</script>

<details class="form-group collapsible-group" open>
	<summary>応答許可ギルド</summary>
	<p class="description-text sub">
		登録したギルド（DiscordサーバーID）でのみ応答します。未許可ギルドでは一切応答・記録しません。
	</p>
	<div class="add-row">
		<input type="text" placeholder="ギルドID（数字）" class="mono" bind:value={guildInput} />
		<Button variant="primary" onclick={add}>追加</Button>
	</div>
	<div class="row-list">
		{#if guilds.length === 0}
			<span class="field-sub">許可ギルドが未登録です。</span>
		{:else}
			{#each guilds as g (g.guild_id)}
				<div class="list-row">
					<span class="field-sub">
						{#if g.guild_name}{g.guild_name}（<span class="mono">{g.guild_id}</span>）{:else}<span
								class="mono">{g.guild_id}</span
							>{/if}
					</span>
					<Button variant="secondary" small onclick={() => remove(g)}>削除</Button>
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
	}
	.add-row .mono {
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
