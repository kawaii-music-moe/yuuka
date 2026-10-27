<script lang="ts">
// 共有ノート（ギルドノート）（旧 assistant-note-guild-select + btn-open-assistant-note
//   + modal-assistant-note + loadAssistantGuildNote + btn-save-assistant-note）。

import { ApiError } from "$lib/api/client";
import { botAttributeApi } from "$lib/api/services";
import { Button, Modal, Select } from "$lib/components/ui";
import { pushToast } from "$lib/stores/toast";
import type { AssistantGuild, GuildNoteResp } from "../config/configTypes";
import { guildLabel, guildOptions } from "../config/configTypes";

interface Props {
	botId: string;
	guilds: AssistantGuild[];
}
let { botId, guilds }: Props = $props();

let selectedGuild = $state("");
let noteContent = $state("");
let open = $state(false);

$effect(() => {
	if (guilds.length === 0) {
		selectedGuild = "";
		return;
	}
	if (!guilds.some((g) => g.guild_id === selectedGuild)) {
		selectedGuild = guilds[0].guild_id;
	}
});

async function loadNote(guildId: string) {
	if (!guildId) {
		noteContent = "";
		return;
	}
	try {
		const res = (await botAttributeApi.getGuildNote(botId, {
			guildId,
		})) as GuildNoteResp;
		noteContent = res.success ? (res.content ?? "") : "";
	} catch {
		noteContent = "";
	}
}

async function openEditor() {
	if (!selectedGuild) {
		pushToast("先に応答許可ギルドを追加してください。", "error");
		return;
	}
	await loadNote(selectedGuild);
	open = true;
}

async function save() {
	if (!selectedGuild) {
		pushToast("先に応答許可ギルドを追加してください。", "error");
		return;
	}
	try {
		const res = await botAttributeApi.setGuildNote({
			botId,
			guildId: selectedGuild,
			content: noteContent,
		});
		if (res.success) {
			open = false;
			pushToast("保存しました。", "success");
		} else {
			pushToast(res.message ?? "保存に失敗しました。", "error");
		}
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
		<h3><span class="material-symbols-outlined header-icon-symbol">description</span>共有ノート（ギルドノート）</h3>
	</div>
	<div class="section-body">
		<div class="note-row">
			<Select
				class="grow"
				ariaLabel="ギルド"
				options={guildOptions(guilds)}
				bind:value={selectedGuild}
			/>
			<Button variant="primary" onclick={openEditor}>編集</Button>
		</div>
	</div>
</section>

<Modal bind:open wide title="共有ノートの編集">
	<p class="field-sub note-modal-guild">
		{guildLabel(guilds, selectedGuild)}（<span class="mono">{selectedGuild}</span>）
	</p>
	<div class="form-group">
		<textarea
			rows="16"
			placeholder="共有ノートの内容（10,000文字まで）"
			bind:value={noteContent}
		></textarea>
	</div>
	<Button variant="primary" block onclick={save}>保存する</Button>
</Modal>

<style>
	.note-row {
		display: flex;
		gap: 12px;
	}
	.note-row :global(.grow) {
		flex-grow: 1;
	}
	/* Modal は body へポータルされ .discord-tab スコープ外のため、ここで直接底上げする */
	.note-modal-guild {
		margin-bottom: 8px;
		font-size: 0.9rem;
		color: var(--text-primary);
	}
	.note-modal-guild .mono {
		font-size: 0.8rem;
		color: var(--color-zinc-muted);
	}
	.mono {
		font-family: var(--font-family-mono);
	}
	textarea {
		width: 100%;
		resize: vertical;
	}
</style>
