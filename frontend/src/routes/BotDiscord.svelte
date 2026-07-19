<script lang="ts">
	// ─────────────────────────────────────────────────────────────────────────
	// Discord連携 タブ（旧 app.js の fetchDiscordSettings / loadAssistantConfig の
	//   Discord 部分 + index.html #tab-discord を移植）。
	//
	// 「汎用モード Discord 設定」カードのみ（owner かつ mcp_assistant プリセット時）。
	//   応答許可ギルド / 利用メンバー / 利用可能ロール / 利用申請 / 共有ノートを子部品に分割。
	//   assistant-config を1回 fetch し、各セクションへ props で配る。操作後は再取得（nonce）。
	//
	//   ※ Discord独自Botトークンカードは #tab-config（BotConfig）に配置されるため、
	//     ここでは扱わない（旧 HTML の構成に忠実）。
	// ─────────────────────────────────────────────────────────────────────────
	import { activeBot } from "$lib/stores/activeBot";
	import { botAttributeApi } from "$lib/api/services";
	import { isDefaultBot } from "./config/configTypes";
	import type {
		AssistantConfigResp,
		AssistantGuild,
		AssistantChannel,
		AssistantMember,
		AssistantRole,
	} from "./config/configTypes";

	import GuildsSection from "./discord/GuildsSection.svelte";
	import ChannelsSection from "./discord/ChannelsSection.svelte";
	import MembersSection from "./discord/MembersSection.svelte";
	import RolesSection from "./discord/RolesSection.svelte";
	import RequestsSection from "./discord/RequestsSection.svelte";
	import NoteSection from "./discord/NoteSection.svelte";

	let guilds = $state<AssistantGuild[]>([]);
	let channels = $state<AssistantChannel[]>([]);
	let members = $state<AssistantMember[]>([]);
	let roles = $state<AssistantRole[]>([]);
	let isAssistantOwner = $state(false);
	// 操作後の再取得トリガ。RequestsSection のリロード同期にも渡す。
	let reloadKey = $state(0);

	const botId = $derived($activeBot?.id ?? "");
	const isAssistant = $derived(($activeBot?.preset ?? "secretary") === "mcp_assistant");

	// activeBot / reloadKey 変更で assistant-config を再取得。
	$effect(() => {
		void $activeBot?.id;
		void reloadKey;
		void load();
	});

	async function load() {
		const id = $activeBot?.id;
		// 汎用モードでないか、デフォルト Bot は対象外。
		if (!id || isDefaultBot(id) || !isAssistant) {
			isAssistantOwner = false;
			guilds = [];
			channels = [];
			members = [];
			roles = [];
			return;
		}
		try {
			const res = (await botAttributeApi.assistantConfig(id)) as AssistantConfigResp;
			if (!res.success) {
				// 403（非オーナー）等 → カード非表示。
				isAssistantOwner = false;
				return;
			}
			isAssistantOwner = true;
			guilds = res.guilds ?? [];
			channels = res.channels ?? [];
			members = res.members ?? [];
			roles = res.roles ?? [];
		} catch {
			isAssistantOwner = false;
		}
	}

	function refresh() {
		reloadKey++;
	}
</script>

<section class="tab-view discord-tab">
	{#if isAssistantOwner && botId}
		<details class="config-card card" open>
			<summary class="column-header badge-right">
				<h3><span class="material-symbols-outlined header-icon-symbol">dns</span>汎用モード Discord 設定</h3>
				<span class="badge badge-accent">OWNER専用</span>
			</summary>
			<p class="description-text">
				サーバー常駐アシスタントの応答許可ギルド・有効化チャンネル（メンション不要）・利用メンバー・共有ノートを設定します。応答にはBot専用のGemini APIキー・独自Discordトークンも必要です（「Bot設定」ページ）。
			</p>

			<GuildsSection {botId} {guilds} onchanged={refresh} />
			<ChannelsSection {botId} {guilds} {channels} onchanged={refresh} />
			<MembersSection {botId} {guilds} {members} onchanged={refresh} />
			<RolesSection {botId} {guilds} {roles} onchanged={refresh} />
			<RequestsSection {botId} {reloadKey} onchanged={refresh} />
			<NoteSection {botId} {guilds} />
		</details>
	{:else}
		<div class="empty-note glass">
			<p>Discord連携設定は「汎用モード」プリセットのBotのオーナーのみ利用できます。</p>
		</div>
	{/if}
</section>

<style>
	.empty-note {
		padding: 24px;
		text-align: center;
		color: var(--color-zinc-muted);
	}
</style>
