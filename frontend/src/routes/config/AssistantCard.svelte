<script lang="ts">
// 汎用モード（MCPアシスタント）設定カード（旧 loadAssistantConfig の Gemini/MCP/usage 部分）。
// mcp_assistant プリセットの owner にのみ親が描画する。

import { ApiError } from "$lib/api/client";
import { botAttributeApi } from "$lib/api/services";
import { Button, confirmDialog } from "$lib/components/ui";
import { pushToast } from "$lib/stores/toast";
import type { AssistantConfigResp } from "./configTypes";

interface Props {
	botId: string;
}
let { botId }: Props = $props();

let data = $state<AssistantConfigResp | null>(null);
let geminiKey = $state("");

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
			"⚠️ 独自のDiscord Botトークンが未設定です。汎用モードは専用のDiscordクライアントとして動作するため、「Discord連携」ページの「Discord 独自Bot設定」から設定してください。",
		);
	if ((data.guilds ?? []).length === 0)
		w.push("⚠️ 応答許可ギルドが未設定です。許可したギルドでのみ応答します。");
	return w;
});

const rateText = $derived.by(() => {
	const rl = data?.rate_limits;
	if (!rl) return "";
	return `レート制限: ユーザー ${rl.userPerMinute}回/分・${rl.userPerDay}回/日、ギルド ${rl.guildPerDay}回/日（Admin設定で変更可）`;
});

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

<details class="config-card card">
	<summary class="column-header badge-right">
		<h3><span class="material-symbols-outlined header-icon-symbol">extension</span>汎用モード設定</h3>
		<span class="badge badge-accent">OWNER専用</span>
	</summary>
	<p class="description-text">
		サーバー常駐アシスタントの動作設定です。応答には「Bot専用のGemini APIキー」「独自Discordトークン」「応答許可ギルド」の設定が必要です。
	</p>

	{#if warnings.length > 0}
		<div class="warnings">
			{#each warnings as w (w)}
				<div class="field-sub warn">{w}</div>
			{/each}
		</div>
	{/if}

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
		<span class="field-sub">
			※あなた個人のAPIキーとは別に管理・暗号化保存されます。空欄で保存すると削除され、Botは応答を停止します。
		</span>
	</details>

	<details class="form-group collapsible-group">
		<summary>利用中のMCPサーバー</summary>
		<p class="description-text mcp-guide">
			MCPサーバーの登録・編集・削除は「MCPサーバー」タブで、表示中のBotごとに行います。
		</p>
		{#if (data?.mcp_servers ?? []).length === 0}
			<span class="field-sub">
				このBot専用のMCPサーバーは未登録です。「MCPサーバー」タブ（このBotを選択した状態）から登録してください。
			</span>
		{:else}
			{#each data?.mcp_servers ?? [] as s (s.id)}
				<div class="mcp-row">
					<span class="mcp-name">{s.name}</span>
					{#if s.system || !s.enabled}
						<span class="field-sub">
							（{[s.system ? "システムレベル・常時利用可" : "", !s.enabled ? "無効化中" : ""].filter(Boolean).join("・")}）
						</span>
					{/if}
				</div>
			{/each}
		{/if}
	</details>

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
</details>

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
		margin: 4px 0 8px;
	}
	.mcp-row {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 4px 0;
	}
	.mcp-name {
		font-size: 0.9rem;
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
