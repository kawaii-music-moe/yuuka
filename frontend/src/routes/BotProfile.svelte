<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// 基本情報と共有（Bot設定 › このBot）。旧 Bot基本設定の「Bot登録名・属性」と
// 「Bot共有管理」をまとめたページ。
//
// 編集できるのは owner|Admin かつ非デフォルト Bot のみ（旧 BotConfig の showOwnerCards と同条件）。
// デフォルト Bot は設定ハブに行自体を出さない（$lib/botTabs）。
// ─────────────────────────────────────────────────────────────────────────
import { activeBot } from "$lib/stores/activeBot";
import { currentUser } from "$lib/stores/session";
import BotAttributeCard from "./config/BotAttributeCard.svelte";
import BotNameCard from "./config/BotNameCard.svelte";
import { type BotAttrView, isOwnerOrAdmin } from "./config/configTypes";
import { fetchBotAttr } from "./config/currentBot";
import ShareCard from "./config/ShareCard.svelte";

let bot = $state<BotAttrView | null>(null);
let loaded = $state(false);
// 保存後に Bot 属性を引き直す nonce。
let reloadNonce = $state(0);

const botId = $derived($activeBot?.id ?? "");
const canEdit = $derived(
	isOwnerOrAdmin(
		bot ?? undefined,
		$currentUser?.discordId ?? "",
		$currentUser?.role,
	),
);

$effect(() => {
	const id = botId;
	void reloadNonce;
	loaded = false;
	void fetchBotAttr(id).then((b) => {
		// Bot 切替の競合防止: 応答が現在選択中の Bot のものでなければ捨てる。
		if (id !== ($activeBot?.id ?? "")) return;
		bot = b;
		loaded = true;
	});
});

function refresh() {
	reloadNonce++;
}
</script>

<section class="tab-view settings-page">
	{#if !loaded}
		<p class="settings-page-note" aria-busy="true">設定を読み込んでいます…</p>
	{:else if bot && canEdit}
		<BotNameCard {botId} {bot} onsaved={refresh} />
		<BotAttributeCard {botId} {bot} onchanged={refresh} />
		<ShareCard {botId} />
	{:else}
		<p class="settings-page-note">この設定は Bot のオーナーだけが変更できます。</p>
	{/if}
</section>
