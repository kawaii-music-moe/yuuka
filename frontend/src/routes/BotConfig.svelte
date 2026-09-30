<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// Bot設定 タブ（旧 app.js の fetchConfigSettings / fetchBotAttributeConfig /
//   loadBotModules / fetchBotShares + index.html #tab-config を移植）。
//
// 手続き的な getElementById().value= を、fetch 結果オブジェクト + 子カードの
//   bind:value/bind:checked へ全置換。可視性ガードは {#if}（旧 classList.add("hidden")）。
//
// カード可視性（旧仕様の忠実移植）:
//   - Discord独自Bot / バックアップ: isSystemDefault && !isAdmin なら非表示。
//   - モジュール選択: アクセス可能な全 Bot（デフォルト含む）で表示。
//   - Bot登録名 / 属性 / 招待 / 共有 / 汎用モード: owner|Admin かつ非デフォルト Bot のみ。
//   - 汎用モード設定: 上記に加え preset==="mcp_assistant" のみ。
//   - アシスタント設定 / 認証情報: 常時表示。
// ─────────────────────────────────────────────────────────────────────────

import { botAttributeApi, settingsApi } from "$lib/api/services";
import { activeBot } from "$lib/stores/activeBot";
import { currentUser } from "$lib/stores/session";
import AssistantCard from "./config/AssistantCard.svelte";
import BackupCard from "./config/BackupCard.svelte";
import BotAttributeCard from "./config/BotAttributeCard.svelte";
import BotInviteCard from "./config/BotInviteCard.svelte";
import BotModulesCard from "./config/BotModulesCard.svelte";
import BotNameCard from "./config/BotNameCard.svelte";
import CredentialsCard from "./config/CredentialsCard.svelte";
import {
	type BotAttrView,
	type BotListResp,
	isDefaultBot,
	isOwnerOrAdmin,
	isSystemDefaultBot,
	type StatusConfig,
	type StatusConfigResponse,
} from "./config/configTypes";
import DiscordTokenCard from "./config/DiscordTokenCard.svelte";
import ShareCard from "./config/ShareCard.svelte";
import UserSettingsCard from "./config/UserSettingsCard.svelte";

let statusConfig = $state<StatusConfig | null>(null);
let currentBot = $state<BotAttrView | null>(null);
// 子カードの再取得を促す nonce（保存後に ++ で再購読）。
let reloadNonce = $state(0);

const botId = $derived($activeBot?.id ?? "");
const isAdmin = $derived($currentUser?.role === "admin");
const userId = $derived($currentUser?.discordId ?? "");

const sysDefault = $derived(isSystemDefaultBot(botId));
const defaultBot = $derived(isDefaultBot(botId));
// 属性系カードの表示条件: 非デフォルト Bot かつ owner|Admin。
const showOwnerCards = $derived(
	!defaultBot &&
		isOwnerOrAdmin(currentBot ?? undefined, userId, $currentUser?.role),
);
// Discord独自Bot / バックアップ: system_default を非管理者が見ている場合のみ隠す。
const showRestricted = $derived(!(sysDefault && !isAdmin));
const isAssistant = $derived(
	(currentBot?.preset ?? $activeBot?.preset) === "mcp_assistant",
);

// activeBot 変更で /api/status と /api/bots を再取得（bot-scoped 追従）。
$effect(() => {
	void $activeBot?.id;
	void reloadNonce;
	void loadStatus();
	void loadBot();
});

async function loadStatus() {
	try {
		const res = (await settingsApi.status()) as StatusConfigResponse;
		statusConfig = res.success ? (res.config ?? null) : null;
	} catch {
		statusConfig = null;
	}
}

async function loadBot() {
	const id = $activeBot?.id;
	if (!id || isDefaultBot(id)) {
		currentBot = null;
		return;
	}
	try {
		const res = (await botAttributeApi.botList()) as BotListResp;
		currentBot = (res.bots ?? []).find((b) => b.id === id) ?? null;
	} catch {
		currentBot = null;
	}
}

// 保存後、依存カードの再取得を促す（Bot一覧・status を引き直す）。
function refresh() {
	reloadNonce++;
}
</script>

<section class="tab-view config-tab">
	{#if showOwnerCards && currentBot}
		<BotNameCard {botId} bot={currentBot} onsaved={refresh} />
	{/if}

	{#if showRestricted}
		<DiscordTokenCard {botId} onsaved={refresh} />
	{/if}

	{#if showOwnerCards && currentBot}
		<BotInviteCard {botId} bot={currentBot} onsynced={refresh} />
		<BotAttributeCard {botId} bot={currentBot} onchanged={refresh} />
	{/if}

	<!-- モジュール選択: アクセス可能な全 Bot（デフォルト含む）で表示。 -->
	{#if botId}
		<BotModulesCard {botId} />
	{/if}

	{#if showOwnerCards && isAssistant}
		<AssistantCard {botId} />
	{/if}

	<UserSettingsCard config={statusConfig} />

	{#if showOwnerCards}
		<ShareCard {botId} />
	{/if}

	{#if showRestricted}
		<BackupCard config={statusConfig} onsaved={refresh} />
	{/if}

	<CredentialsCard {botId} />
</section>

<style>
	.config-tab {
		display: flex;
		flex-direction: column;
		gap: 24px;
	}
	/* details カードの上マージンは gap に集約するため 0 に。 */
	.config-tab :global(.config-card) {
		margin-top: 0 !important;
	}
</style>
