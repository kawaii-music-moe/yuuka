<script lang="ts">
	// 利用メンバー（旧 renderAssistantMembers + loadGuildOptions("member") + btn-add-assistant-member）。
	import { botAttributeApi } from "$lib/api/services";
	import { ApiError } from "$lib/api/client";
	import { pushToast } from "$lib/stores/toast";
	import { confirmDialog, Button } from "$lib/components/ui";
	import { guildLabel } from "../config/configTypes";
	import type {
		AssistantGuild,
		AssistantMember,
		GuildOptionItem,
		GuildOptionsResp,
	} from "../config/configTypes";

	interface Props {
		botId: string;
		guilds: AssistantGuild[];
		members: AssistantMember[];
		onchanged: () => void;
	}
	let { botId, guilds, members, onchanged }: Props = $props();

	let selectedGuild = $state("");
	let selectedMember = $state("");
	let memberInput = $state("");
	let memberOptions = $state<GuildOptionItem[]>([]);
	let available = $state(true);

	// guilds が来たら既定の選択ギルドを合わせ、候補を読み込む。
	$effect(() => {
		if (guilds.length === 0) {
			selectedGuild = "";
			return;
		}
		if (!guilds.some((g) => g.guild_id === selectedGuild)) {
			selectedGuild = guilds[0].guild_id;
		}
	});

	// 選択ギルド変更でメンバー候補を取得（bot 未起動時は手入力に誘導）。
	$effect(() => {
		const gid = selectedGuild;
		if (!gid) {
			memberOptions = [];
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
				memberOptions = [];
				available = false;
				return;
			}
			available = res.available ?? true;
			memberOptions = res.members ?? [];
			selectedMember = "";
		} catch {
			memberOptions = [];
			available = false;
		}
	}

	async function add() {
		const guildId = selectedGuild;
		const userId = (selectedMember || memberInput.trim()).trim();
		if (!guildId) {
			pushToast("先に応答許可ギルドを追加してください。", "error");
			return;
		}
		if (!/^\d{5,25}$/.test(userId)) {
			pushToast("メンバーをプルダウンから選ぶか、ユーザーID（数字）を入力してください。", "error");
			return;
		}
		try {
			const res = await botAttributeApi.setMembers({
				botId,
				guildId,
				userId,
				action: "add",
			});
			if (res.success) {
				memberInput = "";
				selectedMember = "";
				onchanged();
			} else pushToast(res.message ?? "操作に失敗しました。", "error");
		} catch (err) {
			pushToast(err instanceof ApiError ? err.message : "通信エラーが発生しました。", "error");
		}
	}

	async function remove(m: AssistantMember, shift: boolean) {
		if (!shift) {
			const label = m.member_name ? `${m.member_name}（${m.user_id}）` : `ユーザー ${m.user_id}`;
			const ok = await confirmDialog({
				message: `${label} を利用メンバーから削除しますか？`,
				danger: true,
				confirmLabel: "削除",
			});
			if (!ok) return;
		}
		try {
			const res = await botAttributeApi.setMembers({
				botId,
				guildId: m.guild_id,
				userId: m.user_id,
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
	<summary>利用メンバー</summary>
	<p class="description-text sub">
		ギルドごとの利用メンバーを管理します。あなた（owner）は常に利用できます。プルダウンから選ぶか、一覧に無い場合はユーザーIDを直接入力してください。削除ボタンはShiftを押しながらで確認なしに削除できます。
	</p>
	<div class="add-row">
		<select bind:value={selectedGuild} class="min180">
			{#each guilds as g (g.guild_id)}
				<option value={g.guild_id}>{g.guild_name ?? `ギルド ${g.guild_id}`}</option>
			{/each}
		</select>
		<select bind:value={selectedMember} class="min180">
			{#if !available}
				<option value="">（Bot未起動/未参加: ID手入力をご利用ください）</option>
			{:else}
				<option value="">（メンバーを選択）</option>
				{#each memberOptions as m (m.id)}
					<option value={m.id}>{m.name}（{m.id}）</option>
				{/each}
			{/if}
		</select>
		<input type="text" placeholder="または ユーザーID（数字）" class="mono grow" bind:value={memberInput} />
		<Button variant="primary" onclick={add}>追加</Button>
	</div>
	<div class="row-list">
		{#if members.length === 0}
			<span class="field-sub">登録済みの利用メンバーはいません（あなたは常に利用できます）。</span>
		{:else}
			{#each members as m (m.guild_id + ":" + m.user_id)}
				<div class="list-row">
					<span class="field-sub">
						{#if m.member_name}{m.member_name}（<span class="mono">{m.user_id}</span
							>）{:else}<span class="mono">{m.user_id}</span>{/if}
						@ {guildLabel(guilds, m.guild_id)}
					</span>
					<button
						type="button"
						class="btn btn-secondary btn-sm"
						title="Shiftを押しながらで確認なしに削除"
						onclick={(e) => remove(m, e.shiftKey)}
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
