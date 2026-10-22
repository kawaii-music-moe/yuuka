<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// 有効な機能（Bot設定 › このBot）。旧 Bot基本設定の「有効な機能」。
// あなた × このBot（BotModulesCard）。汎用属性の LLM 設定は「LLM機能」（BotLlm）へ分けた。
// ─────────────────────────────────────────────────────────────────────────
import { activeBot } from "$lib/stores/activeBot";
import BotModulesCard from "./config/BotModulesCard.svelte";

const botId = $derived($activeBot?.id ?? "");
</script>

<section class="tab-view settings-page">
	{#if botId}
		<BotModulesCard {botId} />
	{/if}
</section>
