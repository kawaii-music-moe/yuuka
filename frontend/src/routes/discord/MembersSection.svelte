<script lang="ts">
// 利用メンバー（旧 renderAssistantMembers + loadGuildOptions("member") + btn-add-assistant-member）。

import { ApiError } from "$lib/api/client";
import { botAttributeApi } from "$lib/api/services";
import { confirmDialog } from "$lib/components/ui";
import { pushToast } from "$lib/stores/toast";
import type {
	AssistantGuild,
	AssistantMember,
	GuildOptionItem,
	GuildOptionsResp,
} from "../config/configTypes";
import { guildLabel } from "../config/configTypes";
import AddButton from "./AddButton.svelte";
import DeleteButton from "./DeleteButton.svelte";
import OptionCombobox from "./OptionCombobox.svelte";

interface Props {
	botId: string;
	guilds: AssistantGuild[];
	members: AssistantMember[];
	onchanged: () => void;
}
let { botId, guilds, members, onchanged }: Props = $props();

let selectedGuild = $state("");
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
	} catch {
		memberOptions = [];
		available = false;
	}
}

async function add() {
	const guildId = selectedGuild;
	const userId = memberInput.trim();
	if (!guildId) {
		pushToast("先に応答許可ギルドを追加してください。", "error");
		return;
	}
	if (!/^\d{5,25}$/.test(userId)) {
		pushToast(
			"メンバーを候補から選ぶか、ユーザーID（数字）を入力してください。",
			"error",
		);
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
			onchanged();
		} else pushToast(res.message ?? "操作に失敗しました。", "error");
	} catch (err) {
		pushToast(
			err instanceof ApiError ? err.message : "通信エラーが発生しました。",
			"error",
		);
	}
}

async function remove(m: AssistantMember, shift: boolean) {
	if (!shift) {
		const label = m.member_name
			? `${m.member_name}（${m.user_id}）`
			: `ユーザー ${m.user_id}`;
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
		pushToast(
			err instanceof ApiError ? err.message : "通信エラーが発生しました。",
			"error",
		);
	}
}
</script>

<section class="config-card card">
	<div class="column-header">
		<h3><span class="material-symbols-outlined header-icon-symbol">group</span>利用メンバー</h3>
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
				<OptionCombobox
					options={memberOptions.filter(
						(o) => !members.some((m) => m.guild_id === selectedGuild && m.user_id === o.id),
					)}
					bind:value={memberInput}
					placeholder={available ? "メンバー名 または ユーザーID" : "ユーザーID（数字）"}
				/>
			</div>
			<AddButton onclick={add} disabled={!memberInput.trim()} icon="person_add" title="メンバーを追加" />
		</div>
		<div class="row-list">
			{#if members.length === 0}
				<span class="field-sub">登録済みの利用メンバーはいません（あなたは常に利用できます）。</span>
			{:else}
				{#each members as m (m.guild_id + ":" + m.user_id)}
					<div class="list-row">
						<span class="row-label">
							{#if m.member_name}{m.member_name}（<span class="mono">{m.user_id}</span
								>）{:else}<span class="mono">{m.user_id}</span>{/if}
							@ {guildLabel(guilds, m.guild_id)}
						</span>
						<DeleteButton title="Shiftを押しながらで確認なしに削除" onclick={(e) => remove(m, e.shiftKey)} />
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
