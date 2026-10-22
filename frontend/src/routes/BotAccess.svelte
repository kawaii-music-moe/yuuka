<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// 連携の利用許可（Bot設定 › このBot）。「このBotに何を使わせるか」を 1 ページにまとめる。
// 登録そのものは「連携サービス」（IntegratedOverlay）で行い、ここは許可の切り替えと確認だけ。
//
// - MCP サーバー（このBot・共有メンバー間で同期）: 旧 MCPサーバータブの「このBotが利用する
//   MCPサーバー」。付与/解除は integratedApi.grantMcp（共有相手も可・付与は自分のサーバーのみ）。
//   提供ツールは自分が見えるサーバー（mcpApi.list）の分だけ添える。
// - 認証情報（あなた × このBot）: 旧 Bot基本設定の「AI認証情報」（読み取り専用）。
// - Google アカウント（あなた × このBot）: 旧 統合管理の「Bot別 使用アカウント」のこのBot分。
//   overview は所有 Bot だけを返す（許可の対象は owner 本人の Bot のみ）ので、共有Botでは案内だけ出す。
// ─────────────────────────────────────────────────────────────────────────

import { ApiError } from "$lib/api/client";
import { integratedApi, mcpApi } from "$lib/api/services";
import type {
	BotMcpGrantServer,
	BotMcpOwnServer,
} from "$lib/api/services/integratedApi";
import type {
	IntegratedBotView,
	IntegratedGoogleAccountView,
} from "$lib/api/types";
import {
	Button,
	EmptyState,
	Icon,
	ScopeBadge,
	TagChip,
} from "$lib/components/ui";
import { withBasePath } from "$lib/router";
import { activeBot } from "$lib/stores/activeBot";
import { pushToast } from "$lib/stores/toast";
import CredentialsCard from "./config/CredentialsCard.svelte";

const botId = $derived($activeBot?.id ?? "");

// ── MCP（bot-canonical） ──
let botServers = $state<BotMcpGrantServer[]>([]);
let ownGrantable = $state<BotMcpOwnServer[]>([]);
let grantsLoading = $state(false);
let toolsById = $state<Record<number, string[]>>({});

// ── Google（所有 Bot のみ） ──
let ownedBot = $state<IntegratedBotView | null>(null);
let googleAccounts = $state<IntegratedGoogleAccountView[]>([]);
let overviewLoaded = $state(false);

function reportError(e: unknown) {
	pushToast(
		e instanceof ApiError ? e.message : "エラーが発生しました",
		"error",
	);
}

async function loadBotGrants(id: string) {
	grantsLoading = true;
	try {
		const res = await integratedApi.botMcpGrants();
		// Bot 切替の競合防止: 応答が現在選択中の Bot のものでなければ破棄（stale 応答で上書きしない）。
		if (res.bot_id !== ($activeBot?.id ?? "") || res.bot_id !== id) return;
		botServers = res.servers ?? [];
		ownGrantable = res.own_servers ?? [];
	} catch (e) {
		reportError(e);
		botServers = [];
		ownGrantable = [];
	} finally {
		grantsLoading = false;
	}
}

async function loadTools() {
	try {
		const res = await mcpApi.list();
		const next: Record<number, string[]> = {};
		for (const s of res.servers ?? []) {
			next[s.id] = (s.tools ?? []).map((t) => t.name);
		}
		toolsById = next;
	} catch {
		toolsById = {};
	}
}

async function loadOverview(id: string) {
	overviewLoaded = false;
	try {
		const data = await integratedApi.overview();
		if (id !== ($activeBot?.id ?? "")) return;
		ownedBot = (data.bots ?? []).find((b) => b.id === id) ?? null;
		googleAccounts = data.googleAccounts ?? [];
	} catch (e) {
		reportError(e);
		ownedBot = null;
	} finally {
		overviewLoaded = true;
	}
}

$effect(() => {
	const id = botId;
	if (!id) return;
	void loadBotGrants(id);
	void loadOverview(id);
	void loadTools();
});

// このBotへ MCP サーバーを付与/解除する。
async function setGrant(serverId: number, granted: boolean) {
	if (!botId) return;
	try {
		await integratedApi.grantMcp({ botId, serverId, granted });
		await loadBotGrants(botId);
	} catch (e) {
		reportError(e);
	}
}

// このBotで使う Google アカウント（primary / none / account）。
function assignValue(b: IntegratedBotView): string {
	const gs = b.google_setting;
	return gs === "primary" || gs === "none" ? String(gs) : `acct:${gs}`;
}
async function onAssignChange(value: string) {
	if (!botId) return;
	try {
		if (value === "primary")
			await integratedApi.grantGoogle({ botId, mode: "primary" });
		else if (value === "none")
			await integratedApi.grantGoogle({ botId, mode: "none" });
		else
			await integratedApi.grantGoogle({
				botId,
				mode: "account",
				accountId: Number(value.slice(5)),
			});
		pushToast("このBotで使う Google アカウントを変更しました。", "success");
		await loadOverview(botId);
	} catch (e) {
		reportError(e);
	}
}
</script>

<section class="tab-view settings-page">
	<!-- MCP サーバー -->
	<div class="config-card card">
		<div class="column-header badge-right">
			<h3><Icon name="hub" class="header-icon-symbol" />MCP サーバー</h3>
			<ScopeBadge scope="bot" />
		</div>
		<p class="description-text">
			このBotが会話で使う MCP サーバーです。<strong>Botを共有しているメンバー間で共通</strong
			>で、オーナー・共有相手のどちらからでも追加・解除できます（追加できるのは自分が登録したサーバーだけ）。登録は<a
				href={withBasePath("/integrated/mcp")}>連携サービス › MCP サーバー</a
			>で行います。
		</p>
		<div class="access-list">
			{#if botServers.length > 0}
				{#each botServers as s (s.id)}
					<div class="card-item glass access-item">
						<div class="access-item-head">
							<span class="card-title access-item-title">{s.name}</span>
							<span class="badge badge-accent access-badge">
								{s.system ? "システム" : s.mine ? "自分が追加" : "共有相手が追加"}
							</span>
							<span class="status-badge {s.enabled ? 'status-sent' : 'status-cancelled'}"
								>{s.enabled ? "有効" : "無効"}</span
							>
						</div>
						{#if (toolsById[s.id] ?? []).length > 0}
							<div class="access-tools">
								{#each toolsById[s.id] as tool (tool)}
									<TagChip label={tool} />
								{/each}
							</div>
						{/if}
						<div class="access-actions">
							{#if s.system}
								<span class="field-sub">システム登録（全Bot常時利用・解除不可）</span>
							{:else}
								<Button variant="secondary" small onclick={() => setGrant(s.id, false)}
									>このBotから外す</Button
								>
							{/if}
						</div>
					</div>
				{/each}
			{:else if !grantsLoading}
				<EmptyState icon="hub" message="このBotが使う MCP サーバーはまだありません。" />
			{/if}
		</div>

		{#if ownGrantable.length > 0}
			<div class="access-add">
				<span class="field-sub">自分の MCP サーバーをこのBotに追加:</span>
				<div class="access-list">
					{#each ownGrantable as s (s.id)}
						<div class="card-item glass access-item">
							<div class="access-item-head">
								<span class="card-title access-item-title">{s.name}</span>
								{#if s.has_auth}
									<span class="badge badge-accent access-badge">🔑</span>
								{/if}
							</div>
							<div class="access-endpoint" title={s.endpoint_url}>{s.endpoint_url}</div>
							<div class="access-actions">
								<Button variant="primary" small onclick={() => setGrant(s.id, true)}
									>このBotに追加</Button
								>
							</div>
						</div>
					{/each}
				</div>
			</div>
		{/if}
	</div>

	<!-- 認証情報（読み取り専用） -->
	<CredentialsCard {botId} />

	<!-- Google アカウント -->
	<div class="config-card card">
		<div class="column-header badge-right">
			<h3><Icon name="account_circle" class="header-icon-symbol" />Google アカウント</h3>
			<ScopeBadge scope="user_bot" />
		</div>
		{#if !overviewLoaded}
			<p class="field-sub" aria-busy="true">読み込んでいます…</p>
		{:else if ownedBot}
			<p class="description-text">
				カレンダー連携などでこのBotが使う、あなたの Google アカウントです。アカウントの連携・削除は<a
					href={withBasePath("/integrated/google")}>連携サービス › Google アカウント</a
				>で行います。
			</p>
			<div class="form-group access-google">
				<label for="access-google-account">このBotで使うアカウント</label>
				<select
					id="access-google-account"
					value={assignValue(ownedBot)}
					onchange={(e) => onAssignChange((e.currentTarget as HTMLSelectElement).value)}
				>
					<option value="primary">（primaryを使用）</option>
					<option value="none">連携なし</option>
					{#each googleAccounts as a (a.id)}
						<option value="acct:{a.id}">{a.email || `アカウント#${a.id}`}</option>
					{/each}
				</select>
			</div>
		{:else}
			<p class="description-text">
				Google アカウントを選べるのは、自分が所有する Bot だけです。
			</p>
		{/if}
	</div>
</section>

<style>
	.access-list {
		display: flex;
		flex-direction: column;
		gap: 12px;
		margin-top: 12px;
	}
	.access-item {
		flex-direction: column;
		align-items: stretch;
		gap: 8px;
		padding: 14px 16px;
	}
	.access-item-head {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}
	.access-item-title {
		font-size: 0.95rem;
	}
	.access-badge {
		font-size: 0.65rem;
	}
	.access-endpoint {
		font-size: 0.78rem;
		font-family: var(--font-family-mono);
		color: var(--color-zinc-muted);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.access-tools {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
	}
	.access-actions {
		display: flex;
		gap: 6px;
		flex-wrap: wrap;
		margin-top: 4px;
	}
	.access-add {
		margin-top: 16px;
		padding-top: 12px;
		border-top: 1px solid var(--border-divider);
	}
	.access-google {
		max-width: 420px;
		margin-top: 12px;
	}
</style>
