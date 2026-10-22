<script lang="ts">
// チャンネル指定の入力。選択ギルドのチャンネルを guild-options から引き、候補つき入力に渡す。
// Bot 未起動・未参加で候補が引けないときは ID の手入力だけになる。

import { botAttributeApi } from "$lib/api/services";
import type { GuildOptionItem, GuildOptionsResp } from "../config/configTypes";
import OptionCombobox from "./OptionCombobox.svelte";

interface Props {
	botId: string;
	guildId: string;
	value: string;
	/** 候補から外すチャンネルID（登録済みのもの） */
	exclude?: string[];
}
let { botId, guildId, value = $bindable(), exclude = [] }: Props = $props();

let options = $state<GuildOptionItem[]>([]);

$effect(() => {
	const gid = guildId;
	options = [];
	if (!gid) return;
	void load(gid);
});

async function load(gid: string) {
	try {
		const res = (await botAttributeApi.guildOptions(botId, {
			guildId: gid,
		})) as GuildOptionsResp;
		if (gid === guildId && res.success) options = res.channels ?? [];
	} catch {
		/* 候補なし（手入力）で継続 */
	}
}
</script>

<OptionCombobox
	options={options.filter((opt) => !exclude.includes(opt.id))}
	bind:value
	prefix="#"
	placeholder={options.length > 0 ? "チャンネル名 または ID" : "チャンネルID（数字）"}
/>
