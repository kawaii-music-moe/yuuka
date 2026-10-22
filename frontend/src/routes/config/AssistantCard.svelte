<script lang="ts">
// 汎用モード（MCPアシスタント）の AI 設定カード（旧 loadAssistantConfig の Gemini/usage 部分）。
// Bot設定 › 機能と AI に置く。MCP の選択は Bot設定 › 連携の利用許可（BotAccess）へ移した。
// mcp_assistant プリセットの owner にのみ親が描画する。

import { ApiError } from "$lib/api/client";
import { botAttributeApi } from "$lib/api/services";
import { Button, confirmDialog, ScopeBadge } from "$lib/components/ui";
import { pushToast } from "$lib/stores/toast";
import type { AssistantConfigResp } from "./configTypes";

interface Props {
	botId: string;
}
let { botId }: Props = $props();

let data = $state<AssistantConfigResp | null>(null);
let geminiKey = $state("");
let provider = $state("gemini");
let model = $state("");
let savingLlm = $state(false);

$effect(() => {
	void botId;
	void load();
});

async function load() {
	try {
		const res = (await botAttributeApi.assistantConfig(
			botId,
		)) as AssistantConfigResp;
		if (!res.success) {
			data = null;
			return;
		}
		data = res;
		provider = res.llm?.provider ?? "gemini";
		model = res.llm?.model ?? "";
		geminiKey = res.has_gemini_key ? "••••••••••••" : "";
	} catch {
		data = null;
	}
}

const warnings = $derived.by(() => {
	if (!data) return [] as string[];
	const w: string[] = [];
	if (!data.has_gemini_key)
		w.push(
			"⚠️ Bot専用のGemini APIキーが未設定です。設定されるまでこのBotは応答しません。",
		);
	if (!data.has_discord_token)
		w.push(
			"⚠️ 独自のDiscord Botトークンが未設定です。汎用モードは専用のDiscordクライアントとして動作するため、Bot設定の「Discord」→「接続」から設定してください。",
		);
	if ((data.guilds ?? []).length === 0)
		w.push(
			"⚠️ 応答許可ギルドが未設定です。許可したギルドでのみ応答します（Bot設定の「Discord」→「応答する場所」）。",
		);
	return w;
});

const rateText = $derived.by(() => {
	const rl = data?.rate_limits;
	if (!rl) return "";
	return `レート制限: ユーザー ${rl.userPerMinute}回/分・${rl.userPerDay}回/日、ギルド ${rl.guildPerDay}回/日（Admin設定で変更可）`;
});

async function saveLlm(inheritDefaults = false) {
	savingLlm = true;
	try {
		const res = await botAttributeApi.setLlm({
			botId,
			provider,
			model,
			inheritDefaults,
		});
		pushToast(
			res.message ?? "AI設定を保存しました。",
			res.success ? "success" : "error",
		);
		if (res.success) await load();
	} catch (err) {
		pushToast(
			err instanceof ApiError ? err.message : "AI設定を保存できませんでした。",
			"error",
		);
	} finally {
		savingLlm = false;
	}
}

async function saveKey() {
	if (!geminiKey.trim()) {
		const ok = await confirmDialog({
			message: "APIキーを削除しますか？ 削除するとこのBotは応答を停止します。",
			danger: true,
			confirmLabel: "削除する",
		});
		if (!ok) return;
	}
	try {
		const res = await botAttributeApi.setGeminiKey({
			botId,
			apiKey: geminiKey,
		});
		pushToast(
			res.message ?? "保存しました。",
			res.success ? "success" : "error",
		);
		await load();
	} catch (err) {
		pushToast(
			err instanceof ApiError ? err.message : "通信エラーが発生しました。",
			"error",
		);
	}
}
</script>

<section class="config-card card">
	<div class="column-header badge-right">
		<h3><span class="material-symbols-outlined header-icon-symbol">extension</span>AI（汎用モード）</h3>
		<ScopeBadge scope="bot" />
		<span class="badge badge-accent">OWNER専用</span>
	</div>

	{#if warnings.length > 0}
		<div class="warnings">
			{#each warnings as w (w)}
				<div class="field-sub warn">{w}</div>
			{/each}
		</div>
	{/if}

	<div class="form-group">
		<label for="bot-llm-provider">AIプロバイダ</label>
		<select id="bot-llm-provider" bind:value={provider}><option value="gemini">Google Gemini</option></select>
	</div>
	<div class="form-group">
		<label for="bot-llm-model">モデル</label>
		<select id="bot-llm-model" bind:value={model}>
			{#each data?.llm_models ?? [] as name (name)}<option value={name}>{name}</option>{/each}
		</select>
		<span class="field-sub">無料枠・利用上限はGoogle AI Studioで確認できます。</span>
	</div>
	<div class="key-row">
		<Button variant="primary" disabled={!data || savingLlm} onclick={() => saveLlm()}>AI設定を保存</Button>
		<Button disabled={!data || savingLlm} onclick={() => saveLlm(true)}>既定値に戻す</Button>
	</div>
	{#if data?.llm_defaults}<span class="field-sub">既定値: {data.llm_defaults.provider} / {data.llm_defaults.model}</span>{/if}

	<details class="form-group collapsible-group" open>
		<summary>Bot専用 Gemini APIキー（必須）</summary>
		<div class="key-row">
			<input
				type="password"
				placeholder="このBot専用のAPIキーを入力"
				autocomplete="new-password"
				bind:value={geminiKey}
			/>
			<Button variant="primary" onclick={saveKey}>保存</Button>
		</div>
	</details>

	<p class="field-sub mcp-guide">
		このBotが使う MCP サーバーは、Bot設定の「連携の利用許可」で選びます（現在 {(data?.mcp_servers ?? []).length} 件）。
	</p>

	<details class="form-group collapsible-group">
		<summary>利用量（直近14日）</summary>
		<div class="usage">
			{#if (data?.usage ?? []).length === 0}
				<span class="field-sub">まだ利用がありません。</span>
			{:else}
				{#each data?.usage ?? [] as u (u.date)}
					<div class="usage-row">
						<span class="field-sub">{u.date}</span>
						<span>{u.count} 回</span>
					</div>
				{/each}
			{/if}
		</div>
		{#if rateText}<span class="field-sub">{rateText}</span>{/if}
	</details>
</section>

<style>
	.warnings {
		display: flex;
		flex-direction: column;
		gap: 6px;
		margin-top: 8px;
	}
	.warn {
		color: #f59e0b;
	}
	.key-row {
		display: flex;
		gap: 12px;
	}
	.key-row input {
		flex-grow: 1;
	}
	.mcp-guide {
		margin: 12px 0 4px;
	}
	.usage {
		margin-top: 8px;
	}
	.usage-row {
		display: flex;
		justify-content: space-between;
		max-width: 280px;
	}
</style>
