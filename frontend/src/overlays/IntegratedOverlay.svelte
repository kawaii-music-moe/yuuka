<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// 連携サービス（旧 Bot 統合管理。旧 app.js: fetchIntegratedOverview / wireIntegratedForms /
// renderIntCredentials / renderIntMcp / renderIntGoogle）。
// 外部サービス（Google / MCP / 認証情報）の登録と、各登録を使えるBotの切り替えを扱う。
// 再構成（docs/design/settings-restructure.md）で、Bot の起動・停止は Bot一覧とダッシュボードへ、
// 会話履歴のクリアは Bot設定 › 人格へ移した。
//
// 全面書換方針（§11.4 XSS）:
//   - 旧 innerHTML テンプレートリテラル + data-int-* 属性 + 後付けリスナー →
//     {#each} + onclick=/onchange= 直結。手動 intEsc() は撤去（Svelte 自動エスケープ）。
//   - 許可トグルは overview を即時再取得せず楽観更新（旧 wireIntGrantChips の局所再描画）せず、
//     ここではシンプルに toggle 成功後 overview を再取得する（正確性優先・少データ）。
// ─────────────────────────────────────────────────────────────────────────

import {
	credentialApi,
	integratedApi,
	mcpApi,
	settingsApi,
} from "$lib/api/services";
import type {
	IntegratedBotView,
	IntegratedCredentialView,
	IntegratedGoogleAccountView,
	IntegratedMcpServerView,
	McpServerView,
} from "$lib/api/types";
import {
	Button,
	confirmDialog,
	Icon,
	SecretInput,
	Select,
} from "$lib/components/ui";
import {
	googleAssignOptions,
	googleAssignValue,
	googleGrantBody,
} from "$lib/googleAssign";
import { INTEGRATED_SECTIONS } from "$lib/managementSections";
import { isAdmin } from "$lib/stores/session";
import { pushToast, toastError } from "$lib/stores/toast";
import IntCalendarsModal from "./integrated/IntCalendarsModal.svelte";
import IntGrantChips from "./integrated/IntGrantChips.svelte";
import ManagementOverlayShell from "./ManagementOverlayShell.svelte";

let bots = $state<IntegratedBotView[]>([]);
let mcpServers = $state<IntegratedMcpServerView[]>([]);
let credentials = $state<IntegratedCredentialView[]>([]);
let googleAccounts = $state<IntegratedGoogleAccountView[]>([]);
// Bot が使う Google アカウントの選択肢
const assignOptions = $derived(googleAssignOptions(googleAccounts));
let loading = $state(false);

// MCP の詳細（認証の状態・画面付きツール）。overview には無いので mcpApi.list で引く（serverId → view）。
let mcpDetails = $state<Record<number, McpServerView>>({});

// カレンダーモーダル
let calOpen = $state(false);
let calAccount = $state<IntegratedGoogleAccountView | null>(null);

// MCP Apps（ツールの画面）モーダル
let appOpen = $state(false);
let appTarget = $state<{
	serverId: number;
	serverName: string;
	tool: string;
	requiresConfirmation: boolean;
} | null>(null);

// 認証情報フォーム
let credService = $state("");
let credUsername = $state("");
let credPassword = $state("");
let credUrl = $state("");

// MCP フォーム
let mcpName = $state("");
let mcpEndpoint = $state("");
let mcpAuth = $state("");
let mcpConfirm = $state(true);
let mcpScope = $state<"user" | "system">("user");
const MCP_SCOPE_OPTIONS: { value: "user" | "system"; label: string }[] = [
	{ value: "user", label: "ユーザーレベル（自分のみ）" },
	{ value: "system", label: "システムレベル（全ユーザー）" },
];

// ── オーバービュー取得（旧 fetchIntegratedOverview） ──
async function loadOverview() {
	loading = true;
	try {
		const data = await integratedApi.overview();
		bots = data.bots ?? [];
		mcpServers = data.mcpServers ?? [];
		credentials = data.credentials ?? [];
		googleAccounts = data.googleAccounts ?? [];
		void loadMcpDetails();
	} catch (e) {
		toastError(e);
	} finally {
		loading = false;
	}
}

// MCP の詳細（認証の状態・画面付きツール）を取得する。失敗しても一覧は overview で出せる。
async function loadMcpDetails() {
	try {
		const res = await mcpApi.list();
		const next: Record<number, McpServerView> = {};
		for (const s of res.servers ?? []) next[s.id] = s;
		mcpDetails = next;
	} catch {
		mcpDetails = {};
	}
}

// OAuth の認可から戻ってきたとき（/integrated/mcp?mcp_oauth=...）に結果を知らせ、クエリを消す。
$effect(() => {
	const params = new URLSearchParams(window.location.search);
	const result = params.get("mcp_oauth");
	if (!result) return;
	const message = params.get("message") ?? "";
	pushToast(
		message ||
			(result === "success" ? "認可が完了しました。" : "認可に失敗しました。"),
		result === "success" ? "success" : "error",
	);
	params.delete("mcp_oauth");
	params.delete("message");
	const q = params.toString();
	history.replaceState(
		history.state,
		"",
		`${window.location.pathname}${q ? `?${q}` : ""}`,
	);
});

// 初回・再表示時に取得。
$effect(() => {
	void loadOverview();
});

// ── 許可トグル（credential / mcp） ──
async function toggleCred(
	botId: string,
	serviceName: string,
	granted: boolean,
) {
	try {
		await integratedApi.grantCredential({ botId, serviceName, granted });
		await loadOverview();
	} catch (e) {
		toastError(e);
		await loadOverview();
	}
}
async function toggleMcp(botId: string, serverId: number, granted: boolean) {
	try {
		await integratedApi.grantMcp({ botId, serverId, granted });
		await loadOverview();
	} catch (e) {
		toastError(e);
		await loadOverview();
	}
}

// 認証情報 service_name → 許可済み botId 集合。
function credGrantedIds(serviceName: string): Set<string> {
	return new Set(
		bots
			.filter((b) => (b.granted_credentials ?? []).includes(serviceName))
			.map((b) => b.id),
	);
}
function mcpGrantedIds(serverId: number): Set<string> {
	return new Set(
		bots
			.filter((b) => (b.granted_mcp_ids ?? []).includes(serverId))
			.map((b) => b.id),
	);
}

// ── 認証情報 削除（旧 data-int-cred-del） ──
async function deleteCredential(serviceName: string) {
	const ok = await confirmDialog({
		message: `認証情報「${serviceName}」を削除しますか？`,
		danger: true,
		confirmLabel: "削除",
	});
	if (!ok) return;
	try {
		await credentialApi.delete(serviceName);
		await loadOverview();
	} catch (e) {
		toastError(e);
	}
}

// ── 認証情報 登録フォーム（旧 int-cred-form） ──
async function submitCredential(e: SubmitEvent) {
	e.preventDefault();
	try {
		// サーバは serviceName/username/password/url を読む（credential は無視される）。
		await credentialApi.register({
			serviceName: credService.trim(),
			username: credUsername.trim(),
			password: credPassword,
			url: credUrl.trim() || undefined,
			credential: credPassword,
		});
		pushToast("認証情報を登録しました。", "success");
		credService = "";
		credUsername = "";
		credPassword = "";
		credUrl = "";
		await loadOverview();
	} catch (err) {
		toastError(err);
	}
}

// ── MCP 操作（Tool 再取得/toggle/delete/dashboard） ──
// Tool 再取得は旧 Bot設定 › MCPサーバータブから移した（登録済みサーバーの操作はここに集約）。
async function refreshMcpTools(server: IntegratedMcpServerView) {
	try {
		const res = await mcpApi.refresh(server.id);
		pushToast(res.message || "Tool一覧を更新しました。", "success");
		await loadOverview();
	} catch (e) {
		toastError(e);
	}
}
async function toggleMcpServer(server: IntegratedMcpServerView) {
	try {
		await mcpApi.toggle({ id: server.id, enabled: !server.enabled });
		await loadOverview();
	} catch (e) {
		toastError(e);
	}
}
async function deleteMcpServer(server: IntegratedMcpServerView) {
	const ok = await confirmDialog({
		message: "このMCPサーバーを削除しますか？",
		danger: true,
		confirmLabel: "削除",
	});
	if (!ok) return;
	try {
		await mcpApi.delete(server.id);
		await loadOverview();
	} catch (e) {
		toastError(e);
	}
}

// MCP Apps のモーダル。公式 SDK（AppBridge・zod）を含み大きいので、初めて開くときに読み込む。
let McpAppModal = $state<
	typeof import("../routes/mcp/McpAppModal.svelte").default | null
>(null);

// MCP Apps：ツールの画面を開く。
async function openApp(server: IntegratedMcpServerView, tool: string) {
	try {
		McpAppModal ??= (await import("../routes/mcp/McpAppModal.svelte")).default;
	} catch (e) {
		toastError(e);
		return;
	}
	appTarget = {
		serverId: server.id,
		serverName: server.name,
		tool,
		requiresConfirmation: mcpDetails[server.id]?.requires_confirmation ?? true,
	};
	appOpen = true;
}

// 画面（ui:// リソース）を持つツール。
function appTools(serverId: number) {
	return (mcpDetails[serverId]?.tools ?? []).filter((t) => t.ui_resource_uri);
}

// ── OAuth 2.1 認可 ──
// 認可 URL へ画面ごと遷移する。認可サーバーから戻ると /integrated/mcp に結果付きで戻ってくる。
async function startOAuth(server: IntegratedMcpServerView) {
	try {
		const res = await mcpApi.oauthStart(server.id);
		window.location.assign(res.authorizationUrl);
	} catch (e) {
		toastError(e);
	}
}
async function disconnectOAuth(server: IntegratedMcpServerView) {
	const ok = await confirmDialog({
		message: "このMCPサーバーの OAuth 認可を解除しますか？",
		danger: true,
		confirmLabel: "解除",
	});
	if (!ok) return;
	try {
		await mcpApi.oauthDisconnect(server.id);
		pushToast("OAuth の認可を解除しました。", "success");
		await loadOverview();
	} catch (e) {
		toastError(e);
	}
}

/** 認証の状態の表示文言。 */
function authLabel(serverId: number): string {
	const auth = mcpDetails[serverId]?.auth;
	if (!auth) return "";
	if (auth.mode === "bearer") return "トークン認証";
	if (auth.mode === "oauth") return "OAuth 認可済み";
	return auth.authorization_required ? "認可が必要" : "認証なし";
}

// ── MCP 登録フォーム（旧 int-mcp-form） ──
async function submitMcp(e: SubmitEvent) {
	e.preventDefault();
	try {
		await mcpApi.add({
			name: mcpName.trim(),
			endpointUrl: mcpEndpoint.trim(),
			requiresConfirmation: mcpConfirm,
			authCredential: mcpAuth || undefined,
			scope: $isAdmin ? mcpScope : undefined,
		});
		pushToast("MCPサーバーを登録しました。", "success");
		mcpName = "";
		mcpEndpoint = "";
		mcpAuth = "";
		mcpConfirm = true;
		mcpScope = "user";
		await loadOverview();
	} catch (err) {
		toastError(err);
	}
}

// ── Google: primary / delete / calendars / assign ──
async function setPrimary(accountId: number) {
	try {
		await integratedApi.setPrimaryGoogleAccount(accountId);
		await loadOverview();
	} catch (e) {
		toastError(e);
	}
}
async function deleteGoogle(accountId: number) {
	const ok = await confirmDialog({
		message: "このGoogleアカウント連携を解除しますか？",
		danger: true,
		confirmLabel: "解除",
	});
	if (!ok) return;
	try {
		await integratedApi.deleteGoogleAccount(accountId);
		await loadOverview();
	} catch (e) {
		toastError(e);
	}
}
function openCalendars(acct: IntegratedGoogleAccountView) {
	calAccount = acct;
	calOpen = true;
}
async function saveCalendars(payload: {
	accountId: number;
	calendars: string[];
}) {
	try {
		await integratedApi.setGoogleCalendars(payload);
		calOpen = false;
		await loadOverview();
	} catch (e) {
		toastError(e);
	}
}

// Bot 別 使用 Google アカウント割当（primary / none / account）。
// overview はシステム Bot を含まない（案内役のため・issue #143）ので、bots はすべて所有 Bot。
const ownedBots = $derived(bots);
async function onAssignChange(botId: string, value: string) {
	try {
		await integratedApi.grantGoogle(googleGrantBody(botId, value));
		await loadOverview();
	} catch (e) {
		toastError(e);
	}
}

// Google 連携開始（旧 int-google-connect → OAuth URL へ遷移）。
async function connectGoogle() {
	try {
		const r = await settingsApi.googleOAuthUrl();
		if (r.url) window.location.href = r.url;
	} catch (e) {
		toastError(e);
	}
}
</script>

<ManagementOverlayShell
	id="integrated-overlay"
	title="連携サービス"
	base="/integrated"
	sections={INTEGRATED_SECTIONS}
>
	{#snippet children(section)}
<section id="tab-integrated" class="tab-view">
	<!-- Credentials -->
	{#if section === "credentials"}
		<div class="expense-actions-columns int-cols">
			<div class="action-column">
				<form class="int-form" onsubmit={submitCredential}>
					<div class="form-group">
						<label for="int-cred-service">サービス名 *</label>
						<input
							type="text"
							id="int-cred-service"
							bind:value={credService}
							required
							placeholder="例: github"
							pattern="[a-zA-Z0-9_\-]+"
						/>
					</div>
					<div class="form-group">
						<label for="int-cred-username">ユーザー名 / メール *</label>
						<input
							type="text"
							id="int-cred-username"
							bind:value={credUsername}
							required
							autocomplete="off"
						/>
					</div>
					<div class="form-group">
						<label for="int-cred-password">パスワード *</label>
						<SecretInput id="int-cred-password" bind:value={credPassword} required />
					</div>
					<div class="form-group">
						<label for="int-cred-url">ログインURL（任意）</label>
						<input
							type="text"
							id="int-cred-url"
							bind:value={credUrl}
							placeholder="https://..."
						/>
					</div>
					<Button variant="primary" type="submit" block>認証情報を登録</Button>
				</form>
			</div>
			<div class="action-column">
				<div class="int-list">
					{#if credentials.length === 0}
						<p class="description-text">未登録です。</p>
					{:else}
						{#each credentials as c (c.service_name)}
							<div class="glass int-card">
								<div class="int-card-head">
									<div>
										<strong>{c.service_name}</strong>
										<span class="int-card-sub">{c.username}</span>
									</div>
									<Button
										variant="secondary"
										small
										onclick={() => deleteCredential(c.service_name)}>削除</Button
									>
								</div>
								<IntGrantChips
									{bots}
									grantedIds={credGrantedIds(c.service_name)}
									ontoggle={(botId, granted) =>
										toggleCred(botId, c.service_name, granted)}
								/>
							</div>
						{/each}
					{/if}
				</div>
			</div>
		</div>
	{/if}

	<!-- MCP servers -->
	{#if section === "mcp"}
		<div class="expense-actions-columns int-cols">
			<div class="action-column">
				<form class="int-form" onsubmit={submitMcp}>
					<div class="form-group">
						<label for="int-mcp-name">名前 *</label>
						<input type="text" id="int-mcp-name" bind:value={mcpName} required placeholder="例: my-tools" />
					</div>
					<div class="form-group">
						<label for="int-mcp-endpoint">エンドポイントURL *</label>
						<input
							type="url"
							id="int-mcp-endpoint"
							bind:value={mcpEndpoint}
							required
							placeholder="https://example.com/mcp"
							class="int-mono"
						/>
					</div>
					<div class="form-group">
						<label for="int-mcp-auth">認証情報（Bearerトークン等・任意）</label>
						<SecretInput
							id="int-mcp-auth"
							bind:value={mcpAuth}
							placeholder="登録後は表示されません"
						/>
					</div>
					<div class="form-group int-checkbox-row">
						<input type="checkbox" id="int-mcp-confirm" bind:checked={mcpConfirm} class="int-cb" />
						<label for="int-mcp-confirm" class="int-cb-label"
							>Tool実行前にユーザー確認を必須</label
						>
					</div>
					{#if $isAdmin}
						<div class="form-group">
							<label for="int-mcp-scope">登録スコープ (Adminのみ)</label>
							<Select
								id="int-mcp-scope"
								options={MCP_SCOPE_OPTIONS}
								bind:value={mcpScope}
							/>
						</div>
					{/if}
					<Button variant="primary" type="submit" block>MCPサーバーを登録</Button>
				</form>
			</div>
			<div class="action-column">
				<div class="int-list">
					{#if mcpServers.length === 0}
						<p class="description-text">未登録です。</p>
					{:else}
						{#each mcpServers as s (s.id)}
							<div class="glass int-card">
								<div class="int-card-head">
									<div class="int-mcp-info">
										<strong>{s.name}</strong>
										<span class="int-mcp-state" class:on={s.enabled}
											>{s.enabled ? "有効" : "無効"}</span
										>
										{#if authLabel(s.id)}
											<span
												class="int-mcp-state"
												class:warn={mcpDetails[s.id]?.auth.authorization_required &&
													mcpDetails[s.id]?.auth.mode === "none"}>{authLabel(s.id)}</span
											>
										{/if}
										<div class="int-mcp-url">{s.endpoint_url} ・ {s.tools} tools</div>
									</div>
									<div class="int-mcp-actions">
										{#if mcpDetails[s.id]?.can_manage && mcpDetails[s.id]?.auth.mode === "none"}
											<Button variant="secondary" small onclick={() => startOAuth(s)}
												>OAuthで認可</Button
											>
										{/if}
										{#if mcpDetails[s.id]?.can_manage && mcpDetails[s.id]?.auth.mode === "oauth"}
											<Button variant="secondary" small onclick={() => disconnectOAuth(s)}
												>認可を解除</Button
											>
										{/if}
										<Button variant="secondary" small onclick={() => refreshMcpTools(s)}
											>Tool再取得</Button
										>
										<Button variant="secondary" small onclick={() => toggleMcpServer(s)}
											>{s.enabled ? "無効化" : "有効化"}</Button
										>
										<Button variant="secondary" small onclick={() => deleteMcpServer(s)}
											>削除</Button
										>
									</div>
								</div>
								{#if mcpDetails[s.id]?.can_manage && s.enabled && appTools(s.id).length > 0}
									<div class="int-mcp-apps">
										<span class="int-mcp-apps-label">画面:</span>
										{#each appTools(s.id) as t (t.name)}
											<Button
												variant="secondary"
												small
												title={t.description || t.name}
												onclick={() => openApp(s, t.name)}>{t.name}</Button
											>
										{/each}
									</div>
								{/if}
								<IntGrantChips
									{bots}
									grantedIds={mcpGrantedIds(s.id)}
									ontoggle={(botId, granted) => toggleMcp(botId, s.id, granted)}
								/>
							</div>
						{/each}
					{/if}
				</div>
			</div>
		</div>
	{/if}

	<!-- Google accounts -->
	{#if section === "google"}
		<div class="int-google-body">
			<Button variant="primary" onclick={connectGoogle}>
				<Icon name="add" size={16} /> Googleアカウントを連携
			</Button>
			<div class="int-list">
				{#if googleAccounts.length === 0}
					<p class="description-text">連携アカウントはありません。</p>
				{:else}
					{#each googleAccounts as a (a.id)}
						<div class="glass int-ga-row">
							<div>
								<strong>{a.email || "(メール不明)"}</strong>
								{#if a.is_primary}<span class="int-ga-primary">primary</span>{/if}
								<div class="int-ga-sub">同期対象: {a.calendars.length}件</div>
							</div>
							<div class="int-ga-actions">
								{#if !a.is_primary}
									<Button variant="secondary" small onclick={() => setPrimary(a.id)}
										>primaryに</Button
									>
								{/if}
								<Button variant="secondary" small onclick={() => openCalendars(a)}
									>カレンダー</Button
								>
								<Button variant="secondary" small onclick={() => deleteGoogle(a.id)}
									>削除</Button
								>
							</div>
						</div>
					{/each}
				{/if}
			</div>

			{#if googleAccounts.length > 0}
			<p class="description-text int-assign-heading"><strong>各Botで使うアカウント</strong></p>
			<div class="int-assign-list">
				{#if ownedBots.length === 0}
					<p class="description-text">所有Botがありません。</p>
				{:else}
					{#each ownedBots as b (b.id)}
						<div class="int-assign-row">
							<span class="int-assign-name">{b.name}</span>
							<Select
								class="int-assign-select"
								ariaLabel="{b.name} で使うアカウント"
								options={assignOptions}
								value={googleAssignValue(b)}
								onchange={(v) => onAssignChange(b.id, v)}
							/>
						</div>
					{/each}
				{/if}
			</div>
			{/if}
		</div>
	{/if}
</section>
	{/snippet}
</ManagementOverlayShell>

<IntCalendarsModal bind:open={calOpen} account={calAccount} onsave={saveCalendars} />
{#if McpAppModal}
	<McpAppModal bind:open={appOpen} target={appTarget} />
{/if}

<style>
	.int-cols {
		margin-top: 12px;
	}
	.int-form {
		display: flex;
		flex-direction: column;
		gap: 10px;
	}
	.int-list {
		display: flex;
		flex-direction: column;
		gap: 12px;
		max-height: 520px;
		overflow-y: auto;
	}
	.int-card {
		padding: 12px 14px;
		border-radius: 8px;
	}
	.int-card-head {
		display: flex;
		justify-content: space-between;
		align-items: center;
		gap: 8px;
	}
	.int-card-sub {
		font-size: 0.78rem;
		color: var(--color-zinc-muted, #a1a1aa);
	}
	.int-mcp-info {
		min-width: 0;
	}
	.int-mcp-state {
		font-size: 0.72rem;
		color: #71717a;
	}
	.int-mcp-state.on {
		color: #10b981;
	}
	.int-mcp-state.warn {
		color: #f59e0b;
	}
	.int-mcp-apps {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px;
		margin: 8px 0 4px;
	}
	.int-mcp-apps-label {
		font-size: 0.72rem;
		color: var(--color-zinc-muted, #a1a1aa);
	}
	.int-mcp-url {
		font-size: 0.72rem;
		color: var(--color-zinc-muted, #a1a1aa);
		word-break: break-all;
	}
	.int-mcp-actions {
		display: flex;
		gap: 6px;
		flex-shrink: 0;
	}
	.int-mono {
		font-family: var(--font-family-mono);
	}
	.int-checkbox-row {
		display: flex;
		align-items: center;
		gap: 10px;
	}
	.int-cb {
		width: 20px;
		height: 20px;
	}
	.int-cb-label {
		margin: 0;
	}
	.int-google-body {
		margin-top: 12px;
	}
	.int-ga-row {
		display: flex;
		justify-content: space-between;
		align-items: center;
		gap: 8px;
		padding: 12px 14px;
		border-radius: 8px;
	}
	.int-ga-primary {
		font-size: 0.7rem;
		color: #10b981;
	}
	.int-ga-sub {
		font-size: 0.72rem;
		color: var(--color-zinc-muted, #a1a1aa);
	}
	.int-ga-actions {
		display: flex;
		gap: 6px;
	}
	.int-assign-heading {
		margin-top: 16px;
		margin-bottom: 8px;
	}
	.int-assign-list {
		display: flex;
		flex-direction: column;
		gap: 10px;
	}
	.int-assign-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 10px;
	}
	.int-assign-name {
		font-size: 0.85rem;
	}
	.int-assign-row :global(.int-assign-select) {
		min-width: 200px;
	}
</style>
