<script lang="ts">
	// 利用可能ロール（旧 renderAssistantRoles + loadGuildOptions("role") + btn-add-assistant-role）。
	import { botAttributeApi } from "$lib/api/services";
	import { ApiError } from "$lib/api/client";
	import { pushToast } from "$lib/stores/toast";
	import { confirmDialog, Button } from "$lib/components/ui";
	import { guildLabel } from "../config/configTypes";
	import type {
		AssistantGuild,
		AssistantRole,
		GuildOptionItem,
		GuildOptionsResp,
	} from "../config/configTypes";

	interface Props {
		botId: string;
		guilds: AssistantGuild[];
		roles: AssistantRole[];
		onchanged: () => void;
	}
	let { botId, guilds, roles, onchanged }: Props = $props();

	let selectedGuild = $state("");
	let selectedRole = $state("");
	let roleOptions = $state<GuildOptionItem[]>([]);
	let available = $state(true);

	$effect(() => {
		if (guilds.length === 0) {
			selectedGuild = "";
			return;
		}
		if (!guilds.some((g) => g.guild_id === selectedGuild)) {
			selectedGuild = guilds[0].guild_id;
		}
	});

	$effect(() => {
		const gid = selectedGuild;
		if (!gid) {
			roleOptions = [];
			return;
		}
		void loadOptions(gid);
	});

	async function loadOptions(guildId: string) {
		try {
			const res = (await botAttributeApi.guildOptions(botId, {
				guildId,
			})) as GuildOptionsResp;
			if (!res.success) {
				roleOptions = [];
				available = false;
				return;
			}
			available = res.available ?? true;
			roleOptions = res.roles ?? [];
			selectedRole = "";
		} catch {
			roleOptions = [];
			available = false;
		}
	}

	async function add() {
		const guildId = selectedGuild;
		const roleId = selectedRole;
		const roleName = roleOptions.find((r) => r.id === roleId)?.name;
		if (!guildId) {
			pushToast("先に応答許可ギルドを追加してください。", "error");
			return;
		}
		if (!/^\d{5,25}$/.test(roleId)) {
			pushToast(
				"ロールをプルダウンから選択してください（Botが当該ギルドに参加・起動している必要があります）。",
				"error",
			);
			return;
		}
		try {
			const res = await botAttributeApi.setRoles({
				botId,
				guildId,
				roleId,
				roleName,
				action: "add",
			});
			if (res.success) {
				selectedRole = "";
				onchanged();
			} else pushToast(res.message ?? "操作に失敗しました。", "error");
		} catch (err) {
			pushToast(err instanceof ApiError ? err.message : "通信エラーが発生しました。", "error");
		}
	}

	async function remove(r: AssistantRole, shift: boolean) {
		if (!shift) {
			const ok = await confirmDialog({
				message: `ロール ${r.role_name || r.role_id} を利用可能ロールから削除しますか？`,
				danger: true,
				confirmLabel: "削除",
			});
			if (!ok) return;
		}
		try {
			const res = await botAttributeApi.setRoles({
				botId,
				guildId: r.guild_id,
				roleId: r.role_id,
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
	<summary>利用可能ロール</summary>
	<p class="description-text sub">
		許可したDiscordロールの保有者は、個別追加なしで利用できます（判定はメッセージ受信時に行われます）。ギルドを選ぶとロール一覧を取得します。削除ボタンはShiftを押しながらで確認なしに削除できます。
	</p>
	<div class="add-row">
		<select bind:value={selectedGuild} class="min180">
			{#each guilds as g (g.guild_id)}
				<option value={g.guild_id}>{g.guild_name ?? `ギルド ${g.guild_id}`}</option>
			{/each}
		</select>
		<select bind:value={selectedRole} class="min180 grow">
			{#if !available}
				<option value="">（Bot未起動/未参加のため取得できません）</option>
			{:else}
				<option value="">（ロールを選択）</option>
				{#each roleOptions as r (r.id)}
					<option value={r.id}>{r.name}</option>
				{/each}
			{/if}
		</select>
		<Button variant="primary" onclick={add}>追加</Button>
	</div>
	<div class="row-list">
		{#if roles.length === 0}
			<span class="field-sub">許可中のロールはありません。</span>
		{:else}
			{#each roles as r (r.guild_id + ":" + r.role_id)}
				<div class="list-row">
					<span class="field-sub">
						{#if r.role_name}@{r.role_name}{:else}<span class="mono">{r.role_id}</span>{/if}
						@ {guildLabel(guilds, r.guild_id)}
					</span>
					<button
						type="button"
						class="btn btn-secondary btn-sm"
						title="Shiftを押しながらで確認なしに削除"
						onclick={(e) => remove(r, e.shiftKey)}
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
