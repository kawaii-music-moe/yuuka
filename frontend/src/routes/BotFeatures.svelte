<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// 機能と AI（Bot設定 › このBot）。旧 Bot基本設定の「有効な機能」と「汎用モード設定」。
//
// - 有効な機能: あなた × このBot（BotModulesCard）。
// - AI: どのキーとモデルで応答するかをプリセットごとに見せる（どの設定が効いているかを
//   1 か所で分かるようにする。docs/design/settings-restructure.md「指摘事項への対応」）。
//   - 汎用属性: Bot 専用のキーとモデル（AssistantCard・owner|Admin かつ非デフォルト Bot のみ編集）。
//   - それ以外: 話しかけた人の個人キーとモデル（あなたの設定 › AI モデル）。
// ─────────────────────────────────────────────────────────────────────────
import { onMount } from "svelte";
import { settingsApi } from "$lib/api/services";
import { Button, Icon, ScopeBadge } from "$lib/components/ui";
import { navigateTo } from "$lib/router";
import { activeBot } from "$lib/stores/activeBot";
import { currentUser } from "$lib/stores/session";
import AssistantCard from "./config/AssistantCard.svelte";
import BotModulesCard from "./config/BotModulesCard.svelte";
import { type BotAttrView, isOwnerOrAdmin } from "./config/configTypes";
import { fetchBotAttr } from "./config/currentBot";

let bot = $state<BotAttrView | null>(null);
let loaded = $state(false);
let personalModel = $state("");

const botId = $derived($activeBot?.id ?? "");
const isAssistant = $derived(
	(bot?.preset ?? $activeBot?.preset) === "mcp_assistant",
);
const canEditAssistant = $derived(
	!!bot &&
		isOwnerOrAdmin(bot, $currentUser?.discordId ?? "", $currentUser?.role),
);

$effect(() => {
	const id = botId;
	loaded = false;
	void fetchBotAttr(id).then((b) => {
		if (id !== ($activeBot?.id ?? "")) return;
		bot = b;
		loaded = true;
	});
});

onMount(() => {
	settingsApi
		.getGemini()
		.then((res) => {
			personalModel = res.model;
		})
		.catch(() => {
			/* モデル名は表示しないだけ */
		});
});
</script>

<section class="tab-view settings-page">
	{#if botId}
		<BotModulesCard {botId} />
	{/if}

	{#if !loaded}
		<p class="settings-page-note" aria-busy="true">設定を読み込んでいます…</p>
	{:else if isAssistant && canEditAssistant}
		<AssistantCard {botId} />
	{:else if isAssistant}
		<p class="settings-page-note">
			汎用属性のBotは Bot 専用のキーとモデルで応答します。変更できるのは Bot のオーナーだけです。
		</p>
	{:else}
		<section class="config-card card">
			<div class="column-header badge-right">
				<h3><Icon name="psychology" class="header-icon-symbol" />AI</h3>
				<ScopeBadge scope="user" />
			</div>
			<p class="description-text">
				このBotは、話しかけた人の「あなたの設定 › AI モデル」にある API キーとモデルで応答します（共有されたBotでも、各自のキーが使われます）。
			</p>
			{#if personalModel}
				<p class="field-sub">あなたのモデル: {personalModel}</p>
			{/if}
			<div class="ai-actions">
				<Button variant="secondary" onclick={() => navigateTo("/account/ai")}
					>AI モデルの設定を開く</Button
				>
			</div>
		</section>
	{/if}
</section>

<style>
	.ai-actions {
		margin-top: 12px;
	}
</style>
