<script lang="ts">
// 汎用モード（MCPアシスタント）の LLM機能カード（旧 loadAssistantConfig の Gemini 部分）。
// Bot設定 › LLM機能（BotLlm）に置く。MCP の選択は Bot設定 › 連携の利用許可（BotAccess）へ移した。
// mcp_assistant プリセットの owner にのみ親が描画する。

import { ApiError } from "$lib/api/client";
import { botAttributeApi } from "$lib/api/services";
import {
	confirmDialog,
	SecretInput,
	SecretSetMark,
	Select,
} from "$lib/components/ui";
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
const PROVIDER_OPTIONS = [{ value: "gemini", label: "Google Gemini" }];
const modelOptions = $derived(
	(data?.llm_models ?? []).map((m) => ({ value: m, label: m })),
);
let savingLlm = $state(false);
let savingKey = $state(false);
// 保存済みキーは伏せ字で表示し、入力が変わったときだけ保存できる。
const MASK = "••••••••••••";
// 既定値のままなら「既定値に戻す」は押せない。
const isDefaultLlm = $derived(
	!!data?.llm_defaults &&
		provider === data.llm_defaults.provider &&
		model === data.llm_defaults.model,
);
const canSaveKey = $derived(
	!!data && !savingKey && geminiKey !== (data.has_gemini_key ? MASK : ""),
);

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
		geminiKey = res.has_gemini_key ? MASK : "";
	} catch {
		data = null;
	}
}

const warnings = $derived.by(() => {
	if (!data) return [] as string[];
	const w: string[] = [];
	if (!data.has_discord_token)
		w.push(
			"⚠️ 独自のDiscord Botトークンが未設定です。汎用モードは専用のDiscordクライアントとして動作するため、Bot設定の「Discord」→「接続」から設定してください。",
		);
	return w;
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

async function saveKey(e: SubmitEvent) {
	e.preventDefault();
	if (!canSaveKey) return;
	if (!geminiKey.trim()) {
		const ok = await confirmDialog({
			message: "APIキーを削除しますか？ 削除するとこのBotは応答を停止します。",
			danger: true,
			confirmLabel: "削除する",
		});
		if (!ok) return;
	}
	savingKey = true;
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
	} finally {
		savingKey = false;
	}
}
</script>

<section class="config-card card plain-fields">
	{#if warnings.length > 0}
		<div class="warnings">
			{#each warnings as w (w)}
				<div class="field-sub warn">{w}</div>
			{/each}
		</div>
	{/if}

	<div class="field">
		<label class="field-label" for="bot-llm-provider">LLMプロバイダ</label>
		<div class="field-row">
			<div class="field-input">
				<Select
					id="bot-llm-provider"
					options={PROVIDER_OPTIONS}
					bind:value={provider}
					disabled={!data || savingLlm}
					onchange={() => saveLlm()}
				/>
			</div>
			<span class="row-spacer" aria-hidden="true"></span>
		</div>
	</div>

	<div class="field">
		<label class="field-label" for="bot-llm-model">モデル</label>
		<div class="field-row">
			<div class="field-input">
				<Select
					id="bot-llm-model"
					options={modelOptions}
					bind:value={model}
					disabled={!data || savingLlm}
					onchange={() => saveLlm()}
				/>
			</div>
			<button
				type="button"
				class="icon-action"
				aria-label="既定値に戻す"
				title={data?.llm_defaults
					? `既定値に戻す（${data.llm_defaults.provider} / ${data.llm_defaults.model}）`
					: "既定値に戻す"}
				disabled={!data || savingLlm || isDefaultLlm}
				onclick={() => saveLlm(true)}
				><span class="material-symbols-outlined" aria-hidden="true">restart_alt</span></button>
		</div>
		<span class="field-sub">無料枠・利用上限はGoogle AI Studioで確認できます。</span>
	</div>

	<form class="field" onsubmit={saveKey}>
		<label class="field-label" for="bot-gemini-key">API キー<span class="required" aria-label="必須">*</span><SecretSetMark set={data?.has_gemini_key} /></label>
		<div class="field-row">
			<div class="field-input">
				<SecretInput
					id="bot-gemini-key"
					placeholder="このBot専用のGemini APIキーを入力"
					bind:value={geminiKey}
					disabled={savingKey}
				/>
			</div>
			<button
				type="submit"
				class="icon-action"
				disabled={!canSaveKey}
				aria-label="API キーを保存"
				title="API キーを保存"
			><span class="material-symbols-outlined" aria-hidden="true">check</span></button>
		</div>
	</form>
</section>

<style>
	/* 入力欄は Bot登録名カード（BotNameCard）と同じ見た目にそろえる。 */
	.warnings {
		display: flex;
		flex-direction: column;
		gap: 6px;
		margin-top: 8px;
	}
	.warn {
		color: #f59e0b;
	}
	.field {
		margin-top: 12px;
	}
	.field-label {
		display: block;
		margin-bottom: 4px;
		font-size: 0.85rem;
		color: var(--text-secondary);
	}
	.field-row {
		display: flex;
		gap: 12px;
	}
	.field-input {
		position: relative;
		flex: 1;
		min-width: 0;
	}
	.field-input :global(input),
	.field-input :global(.inline-select) {
		width: 100%;
		box-sizing: border-box;
	}
	.required {
		margin-left: 2px;
		color: #ef4444;
		font-weight: 700;
	}
	.row-spacer {
		width: 40px;
		flex-shrink: 0;
	}
</style>
