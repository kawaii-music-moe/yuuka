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
//   - Bot登録名・属性 / 招待 / 共有 / 汎用モード: owner|Admin かつ非デフォルト Bot のみ。
//   - 汎用モード設定: 上記に加え preset==="mcp_assistant" のみ。
//   - アシスタント設定 / 認証情報: 常時表示。
// ─────────────────────────────────────────────────────────────────────────

import { botAttributeApi, settingsApi } from "$lib/api/services";
import { Icon } from "$lib/components/ui";
import { navigateTo, page } from "$lib/router";
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
let botLoaded = $state(false);
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
	botLoaded = false;
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
		botLoaded = true;
		return;
	}
	try {
		const res = (await botAttributeApi.botList()) as BotListResp;
		currentBot = (res.bots ?? []).find((b) => b.id === id) ?? null;
	} catch {
		currentBot = null;
	} finally {
		botLoaded = true;
	}
}

// 保存後、依存カードの再取得を促す（Bot一覧・status を引き直す）。
function refresh() {
	reloadNonce++;
}

type SectionId =
	| "bot-profile"
	| "bot-name"
	| "discord-token"
	| "bot-invite"
	| "bot-attributes"
	| "modules"
	| "assistant-mode"
	| "assistant-settings"
	| "sharing"
	| "backup"
	| "credentials";

type Section = { id: SectionId; label: string; desc: string };

const sections = $derived.by((): Section[] => {
	const items: Section[] = [];
	if (showOwnerCards && currentBot) {
		items.push(
			{
				id: "bot-profile",
				label: "Bot登録名・属性",
				desc: "管理画面に表示する名前とBotの機能セット",
			},
			{
				id: "bot-invite",
				label: "招待リンク・プロフィール",
				desc: "Discordへの導入リンクとプロフィール",
			},
		);
	}
	if (showRestricted) {
		items.push({
			id: "discord-token",
			label: "Discord 独自Bot",
			desc: "Discord Botトークンの設定",
		});
	}
	if (botId) {
		items.push({
			id: "modules",
			label: "有効な機能",
			desc: "このBotで使う機能を選択",
		});
	}
	if (showOwnerCards && isAssistant) {
		items.push({
			id: "assistant-mode",
			label: "汎用モード設定",
			desc: "AIアシスタントの専用設定",
		});
	}
	items.push({
		id: "assistant-settings",
		label: "アシスタント設定",
		desc: "通知・タイムゾーンなどの個人設定",
	});
	if (showOwnerCards) {
		items.push({
			id: "sharing",
			label: "Bot共有管理",
			desc: "Botを利用できるメンバーを管理",
		});
	}
	if (showRestricted) {
		items.push({
			id: "backup",
			label: "Google Drive バックアップ",
			desc: "バックアップの保存先と実行間隔",
		});
	}
	items.push({
		id: "credentials",
		label: "AI認証情報",
		desc: "Botが利用できる認証情報を確認",
	});
	return items;
});

const requestedSection = $derived(
	$page.searchParams.get("section") as SectionId | null,
);
const selectedId = $derived(
	requestedSection === "bot-name" || requestedSection === "bot-attributes"
		? "bot-profile"
		: requestedSection,
);
const selectedSection = $derived(
	sections.find((section) => section.id === selectedId) ?? null,
);

function openSection(id: SectionId) {
	navigateTo(`/bot/config?section=${id}`);
}

function backToSections() {
	navigateTo("/bot/config");
}
</script>

<section class="tab-view config-tab">
	{#if selectedId && !selectedSection && !botLoaded && !defaultBot}
		<div class="config-loading" aria-busy="true">設定を読み込んでいます…</div>
	{:else if selectedId && selectedSection}
		<div class="config-detail-page">
			<button type="button" class="back-to-config" onclick={backToSections}>
				<Icon name="arrow_back" size={18} />
				設定一覧に戻る
			</button>

			{#if selectedId === "bot-profile" && currentBot}
				<BotNameCard {botId} bot={currentBot} onsaved={refresh} />
				<BotAttributeCard {botId} bot={currentBot} onchanged={refresh} />
			{:else if selectedId === "discord-token"}
				<DiscordTokenCard {botId} onsaved={refresh} />
			{:else if selectedId === "bot-invite" && currentBot}
				<BotInviteCard {botId} bot={currentBot} onsynced={refresh} />
			{:else if selectedId === "modules"}
				<BotModulesCard {botId} />
			{:else if selectedId === "assistant-mode"}
				<AssistantCard {botId} />
			{:else if selectedId === "assistant-settings"}
				<UserSettingsCard config={statusConfig} />
			{:else if selectedId === "sharing"}
				<ShareCard {botId} />
			{:else if selectedId === "backup"}
				<BackupCard config={statusConfig} onsaved={refresh} />
			{:else if selectedId === "credentials"}
				<CredentialsCard {botId} />
			{/if}
		</div>
	{:else if selectedId}
		<div class="config-not-found">
			<p>この設定は利用できないか、閲覧権限がありません。</p>
			<button type="button" class="back-to-config" onclick={backToSections}>
				<Icon name="arrow_back" size={18} />
				設定一覧に戻る
			</button>
		</div>
	{:else}
		<nav class="config-navigation" aria-label="Bot基本設定">
			{#each sections as section (section.id)}
				<button
					type="button"
					class="hover-lift config-navigation-card"
					onclick={() => openSection(section.id)}
				>
					<span class="config-navigation-text">
						<span class="config-navigation-label">{section.label}</span>
						<span class="config-navigation-desc">{section.desc}</span>
					</span>
				</button>
			{/each}
		</nav>
	{/if}
</section>

<style>
	.config-tab {
		display: flex;
		flex-direction: column;
		gap: 16px;
	}
	.config-navigation {
		display: flex;
		flex-direction: column;
	}
	.config-navigation-card {
		display: flex;
		align-items: center;
		gap: 14px;
		width: 100%;
		padding: 16px 12px;
		font: inherit;
		color: var(--text-primary);
		text-align: left;
		cursor: pointer;
		background: none;
		border: 0;
		border-bottom: 1px solid var(--border-divider);
		transition: background-color 0.15s ease;
	}
	.config-navigation-card:hover,
	.config-navigation-card:focus-visible {
		background-color: var(--surface-1dp);
	}
	.config-navigation-text {
		display: flex;
		flex-direction: column;
		gap: 4px;
		min-width: 0;
	}
	.config-navigation-label {
		font-weight: 600;
	}
	.config-navigation-desc {
		font-size: 0.82rem;
		color: var(--text-secondary);
	}
	.config-detail-page {
		display: flex;
		flex-direction: column;
		gap: 16px;
	}
	.back-to-config {
		display: inline-flex;
		align-items: center;
		align-self: flex-start;
		gap: 6px;
		padding: 0;
		font: inherit;
		color: var(--text-secondary);
		cursor: pointer;
		background: none;
		border: 0;
	}
	.back-to-config:hover {
		color: var(--text-primary);
		text-decoration: underline;
		text-underline-offset: 4px;
	}
	.config-loading,
	.config-not-found {
		padding: 24px 0;
	}
	/* 個別ページではカード上端の余白を親の gap に集約する。 */
	.config-tab :global(.config-card) {
		margin-top: 0 !important;
	}
</style>
