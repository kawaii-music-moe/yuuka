<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// Discord（Bot設定 › Discord）。旧 app.js の fetchDiscordSettings / loadAssistantConfig の
//   Discord 部分 + index.html #tab-discord を移植し、再構成で旧 Bot基本設定の
//   「Discord 独自Bot」「招待リンク・プロフィール」もここへ集めた。
//
// 項目が多い汎用属性（owner）はページ内タブで分ける:
//   接続（トークン・招待リンク）／応答する場所（ギルド・チャンネル・ミュート）／
//   応答する相手（メンバー・ロール・利用申請）／ギルドノート。
// 秘書属性・非オーナーは「接続」だけなのでタブを出さない。
// 汎用モードの各セクションは assistant-config を1回 fetch して props で配る。操作後は再取得（nonce）。
// ─────────────────────────────────────────────────────────────────────────

import { onDestroy } from "svelte";
import { botAttributeApi } from "$lib/api/services";
import { PageTabs } from "$lib/components/ui";
import { activeBot } from "$lib/stores/activeBot";
import { headerExtra } from "$lib/stores/headerExtra";
import { currentUser, isAdmin } from "$lib/stores/session";
import BotInviteCard from "./config/BotInviteCard.svelte";
import type {
	AssistantChannel,
	AssistantConfigResp,
	AssistantGuild,
	AssistantMember,
	AssistantRole,
	BotAttrView,
} from "./config/configTypes";
import {
	isDefaultBot,
	isOwnerOrAdmin,
	isSystemDefaultBot,
} from "./config/configTypes";
import { fetchBotAttr } from "./config/currentBot";
import DiscordTokenCard from "./config/DiscordTokenCard.svelte";
import ChannelsSection from "./discord/ChannelsSection.svelte";
import GuildsSection from "./discord/GuildsSection.svelte";
import MembersSection from "./discord/MembersSection.svelte";
import MutedChannelsSection from "./discord/MutedChannelsSection.svelte";
import NoteSection from "./discord/NoteSection.svelte";
import RequestsSection from "./discord/RequestsSection.svelte";
import RolesSection from "./discord/RolesSection.svelte";

const TABS = [
	{ id: "connection", label: "接続" },
	{ id: "places", label: "応答する場所" },
	{ id: "people", label: "応答する相手" },
	{ id: "note", label: "ギルドノート" },
];
let view = $state("connection");

let bot = $state<BotAttrView | null>(null);
let guilds = $state<AssistantGuild[]>([]);
let channels = $state<AssistantChannel[]>([]);
let mutedChannels = $state<AssistantChannel[]>([]);
let members = $state<AssistantMember[]>([]);
let roles = $state<AssistantRole[]>([]);
let isAssistantOwner = $state(false);
// 操作後の再取得トリガ。RequestsSection のリロード同期にも渡す。
let reloadKey = $state(0);

const botId = $derived($activeBot?.id ?? "");
const isAssistant = $derived(
	($activeBot?.preset ?? "secretary") === "mcp_assistant",
);
// トークン: system_default を非管理者が見ている場合だけ隠す（旧 BotConfig の showRestricted）。
const showToken = $derived(!(isSystemDefaultBot(botId) && !$isAdmin));
// 招待リンク: owner|Admin かつ非デフォルト Bot（旧 BotConfig の showOwnerCards）。
const showInvite = $derived(
	!!bot &&
		!isDefaultBot(botId) &&
		isOwnerOrAdmin(bot, $currentUser?.discordId ?? "", $currentUser?.role),
);

// activeBot 変更で Bot 属性と assistant-config を再取得。
$effect(() => {
	const id = botId;
	void fetchBotAttr(id).then((b) => {
		if (id === ($activeBot?.id ?? "")) bot = b;
	});
});
$effect(() => {
	void $activeBot?.id;
	void reloadKey;
	void loadAssistant();
});

async function loadAssistant() {
	const id = $activeBot?.id;
	// 汎用モードでないか、デフォルト Bot は対象外。
	if (!id || isDefaultBot(id) || !isAssistant) {
		isAssistantOwner = false;
		guilds = [];
		channels = [];
		mutedChannels = [];
		members = [];
		roles = [];
		return;
	}
	try {
		const res = (await botAttributeApi.assistantConfig(
			id,
		)) as AssistantConfigResp;
		if (!res.success) {
			// 403（非オーナー）等 → 汎用モードのタブを出さない。
			isAssistantOwner = false;
			return;
		}
		isAssistantOwner = true;
		guilds = res.guilds ?? [];
		channels = res.channels ?? [];
		mutedChannels = res.muted_channels ?? [];
		members = res.members ?? [];
		roles = res.roles ?? [];
	} catch {
		isAssistantOwner = false;
	}
}

function refresh() {
	reloadKey++;
}

function refreshBot() {
	const id = botId;
	void fetchBotAttr(id).then((b) => {
		if (id === ($activeBot?.id ?? "")) bot = b;
	});
}

// 汎用モードのタブが無いときは常に「接続」を出す。
const shown = $derived(isAssistantOwner ? view : "connection");

// タブはシェルのヘッダー（スコープバッジの位置）に出す。タブが無いときも空で差し込み、
// バッジは出さない。
headerExtra.set(headerTabs);
onDestroy(() => headerExtra.set(null));
</script>

{#snippet headerTabs()}
	{#if isAssistantOwner && botId}
		<PageTabs tabs={TABS} bind:active={view} label="Discord の設定" flush />
	{/if}
{/snippet}

<section class="tab-view settings-page discord-tab">
	{#if shown === "connection"}
		{#if showToken}
			<DiscordTokenCard {botId} onsaved={refreshBot} />
		{/if}
		{#if showInvite && bot}
			<BotInviteCard {botId} {bot} onsynced={refreshBot} />
		{/if}
		{#if !showToken && !showInvite}
			<p class="settings-page-note">このBotの Discord 設定は管理者だけが変更できます。</p>
		{/if}
		{#if isAssistant && !isAssistantOwner}
			<p class="settings-page-note">
				応答する場所・相手の設定は、汎用属性のBotのオーナーだけが変更できます。
			</p>
		{/if}
	{:else if shown === "places"}
		<GuildsSection {botId} {guilds} onchanged={refresh} />
		<!-- チャンネルはギルドを選んで追加するので、ギルド未登録の間は出さない -->
		{#if guilds.length > 0}
			<ChannelsSection {botId} {guilds} {channels} onchanged={refresh} />
			<MutedChannelsSection {botId} {guilds} {mutedChannels} onchanged={refresh} />
		{/if}
	{:else if shown === "people"}
		<MembersSection {botId} {guilds} {members} onchanged={refresh} />
		<RolesSection {botId} {guilds} {roles} onchanged={refresh} />
		<RequestsSection {botId} {reloadKey} onchanged={refresh} />
	{:else if shown === "note"}
		<NoteSection {botId} {guilds} />
	{/if}
</section>
