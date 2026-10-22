<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// LLM機能（Bot設定 › このBot・汎用属性のみ）。旧 Bot基本設定の「汎用モード設定」。
// Bot 専用のキーとモデル（AssistantCard）は owner|Admin だけが編集できる。
// それ以外の属性の Bot は話しかけた人の個人キー（あなたの設定 › AI モデル）で応答する。
// ─────────────────────────────────────────────────────────────────────────
import { activeBot } from "$lib/stores/activeBot";
import { currentUser } from "$lib/stores/session";
import AssistantCard from "./config/AssistantCard.svelte";
import { type BotAttrView, isOwnerOrAdmin } from "./config/configTypes";
import { fetchBotAttr } from "./config/currentBot";

let bot = $state<BotAttrView | null>(null);
let loaded = $state(false);

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
</script>

<section class="tab-view settings-page">
	{#if loaded && isAssistant && canEditAssistant}
		<AssistantCard {botId} />
	{:else if loaded && isAssistant}
		<p class="settings-page-note">
			汎用属性のBotは Bot 専用のキーとモデルで応答します。変更できるのは Bot のオーナーだけです。
		</p>
	{:else if loaded}
		<p class="settings-page-note">LLM機能の設定は汎用属性のBotで使えます。</p>
	{/if}
</section>
