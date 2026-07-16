<script lang="ts">
	// ─────────────────────────────────────────────────────────────────────────
	// Bot 統合管理オーバービュー（旧 app.js: fetchIntegratedOverview / wireIntegratedForms /
	// renderIntBots / renderIntCredentials / renderIntMcp / renderIntGoogle / closeIntMenus）。
	//
	// 全面書換方針（§11.4 XSS）:
	//   - 旧 innerHTML テンプレートリテラル + data-int-* 属性 + 後付けリスナー →
	//     {#each} + onclick=/onchange= 直結。手動 intEsc() は撤去（Svelte 自動エスケープ）。
	//   - ⋮ メニューは openMenuId を単一 state で保持し、window クリックで閉じる（旧 closeIntMenus）。
	//   - 起動/停止/再起動後は少し待って overview を再取得（Discord 接続確立待ち）。
	//   - 許可トグルは overview を即時再取得せず楽観更新（旧 wireIntGrantChips の局所再描画）せず、
	//     ここではシンプルに toggle 成功後 overview を再取得する（正確性優先・少データ）。
	// ─────────────────────────────────────────────────────────────────────────
	import { integratedApi, credentialApi, settingsApi, mcpApi } from "$lib/api/services";
	import { ApiError } from "$lib/api/client";
	import { pushToast } from "$lib/stores/toast";
	import { confirmDialog } from "$lib/components/ui";
	import { Button, Icon, StatusChip } from "$lib/components/ui";
	import { isAdmin } from "$lib/stores/session";
	import { navigateTo } from "$lib/router";
	import type {
		IntegratedBotView,
		IntegratedMcpServerView,
		IntegratedCredentialView,
		IntegratedGoogleAccountView,
	} from "$lib/api/types";
	import type { ChipStatus } from "$lib/components/ui";

	import IntGrantChips from "./integrated/IntGrantChips.svelte";
	import IntCalendarsModal from "./integrated/IntCalendarsModal.svelte";
	import McpDashboardModal from "../routes/mcp/McpDashboardModal.svelte";

	let bots = $state<IntegratedBotView[]>([]);
	let mcpServers = $state<IntegratedMcpServerView[]>([]);
	let credentials = $state<IntegratedCredentialView[]>([]);
	let googleAccounts = $state<IntegratedGoogleAccountView[]>([]);
	let loading = $state(false);

	// dashboard 提供有無キャッシュ（serverId → available）
	let dashAvailable = $state<Record<number, boolean>>({});

	// ⋮ メニュー開閉（単一のみ開く）
	let openMenuId = $state<string | null>(null);

	// カレンダーモーダル
	let calOpen = $state(false);
	let calAccount = $state<IntegratedGoogleAccountView | null>(null);

	// MCP ダッシュボードモーダル
	let dashOpen = $state(false);
	let dashServer = $state<{ id: number; name: string } | null>(null);

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

	function reportError(e: unknown) {
		pushToast(e instanceof ApiError ? e.message : "エラーが発生しました", "error");
	}

	// ── オーバービュー取得（旧 fetchIntegratedOverview） ──
	async function loadOverview() {
		loading = true;
		try {
			const data = await integratedApi.overview();
			bots = data.bots ?? [];
			mcpServers = data.mcpServers ?? [];
			credentials = data.credentials ?? [];
			googleAccounts = data.googleAccounts ?? [];
			void probeDashboards();
		} catch (e) {
			reportError(e);
		} finally {
			loading = false;
		}
	}

	// dashboard 提供有無を各 MCP サーバについて判定（旧 fire-and-forget probe）。
	// overview には含まれないため mcpApi.dashboardStatus を個別に叩く。
	async function probeDashboards() {
		const next: Record<number, boolean> = {};
		await Promise.all(
			mcpServers.map(async (s) => {
				try {
					const r = await mcpApi.dashboardStatus(s.id);
					next[s.id] = r.available === true;
				} catch {
					next[s.id] = false;
				}
			}),
		);
		dashAvailable = next;
	}

	// 初回・再表示時に取得。
	$effect(() => {
		void loadOverview();
	});

	// ── Bot 起動/停止/再起動（旧 intBotAction） ──
	async function botAction(action: "start" | "stop" | "restart", botId: string) {
		openMenuId = null;
		try {
			if (action === "start") await integratedApi.startBot(botId);
			else if (action === "stop") await integratedApi.stopBot(botId);
			else await integratedApi.restartBot(botId);
		} catch (e) {
			reportError(e);
		}
		// stop は即時反映、start/restart は接続確立を待って少し遅らせて再取得。
		setTimeout(loadOverview, action === "stop" ? 300 : 1500);
	}

	// ── 会話履歴クリア（旧 intClearHistory） ──
	async function clearHistory(botId: string) {
		openMenuId = null;
		const ok = await confirmDialog({
			message:
				"このBotとの会話履歴をクリアしますか？\n次のメッセージから新しい会話になります（永続ログは保持され、検索・監査では引き続き利用できます）。",
			danger: true,
			confirmLabel: "クリア",
		});
		if (!ok) return;
		try {
			await integratedApi.clearHistory(botId);
			pushToast("会話履歴をクリアしました。", "success");
		} catch (e) {
			reportError(e);
		}
	}

	function toggleMenu(botId: string) {
		openMenuId = openMenuId === botId ? null : botId;
	}

	// ── Bot の状態チップ（旧 renderIntBots のバッジ分岐） ──
	function botChip(b: IntegratedBotView): { status: ChipStatus; label: string } {
		if (b.suspended) return { status: "stopped", label: "停止中(管理者)" };
		if (b.connected) return { status: "running", label: "稼働中" };
		if (b.running) return { status: "pending", label: "接続中…" };
		if (!b.has_token && !b.is_system_default)
			return { status: "unset", label: "トークン未設定" };
		return { status: "unset", label: "停止" };
	}

	// ── 許可トグル（credential / mcp） ──
	async function toggleCred(botId: string, serviceName: string, granted: boolean) {
		try {
			await integratedApi.grantCredential({ botId, serviceName, granted });
			await loadOverview();
		} catch (e) {
			reportError(e);
			await loadOverview();
		}
	}
	async function toggleMcp(botId: string, serverId: number, granted: boolean) {
		try {
			await integratedApi.grantMcp({ botId, serverId, granted });
			await loadOverview();
		} catch (e) {
			reportError(e);
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
			reportError(e);
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
			reportError(err);
		}
	}

	// ── MCP 操作（toggle/delete/dashboard） ──
	async function toggleMcpServer(server: IntegratedMcpServerView) {
		try {
			await mcpApi.toggle({ id: server.id, enabled: !server.enabled });
			await loadOverview();
		} catch (e) {
			reportError(e);
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
			reportError(e);
		}
	}
	function openDashboard(server: IntegratedMcpServerView) {
		dashServer = { id: server.id, name: server.name };
		dashOpen = true;
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
			reportError(err);
		}
	}

	// ── Google: primary / delete / calendars / assign ──
	async function setPrimary(accountId: number) {
		try {
			await integratedApi.setPrimaryGoogleAccount(accountId);
			await loadOverview();
		} catch (e) {
			reportError(e);
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
			reportError(e);
		}
	}
	function openCalendars(acct: IntegratedGoogleAccountView) {
		calAccount = acct;
		calOpen = true;
	}
	async function saveCalendars(payload: { accountId: number; calendars: string[] }) {
		try {
			await integratedApi.setGoogleCalendars(payload);
			calOpen = false;
			await loadOverview();
		} catch (e) {
			reportError(e);
		}
	}

	// Bot 別 使用 Google アカウント割当（primary / none / account）。
	const ownedBots = $derived(bots.filter((b) => !b.is_system_default));
	function assignValue(b: IntegratedBotView): string {
		const gs = b.google_setting;
		return gs === "primary" || gs === "none" ? String(gs) : `acct:${gs}`;
	}
	async function onAssignChange(botId: string, value: string) {
		try {
			let body:
				| { botId: string; mode: "primary" }
				| { botId: string; mode: "none" }
				| { botId: string; mode: "account"; accountId: number };
			if (value === "primary") body = { botId, mode: "primary" };
			else if (value === "none") body = { botId, mode: "none" };
			else body = { botId, mode: "account", accountId: Number(value.slice(5)) };
			await integratedApi.grantGoogle(body);
			await loadOverview();
		} catch (e) {
			reportError(e);
		}
	}

	// Google 連携開始（旧 int-google-connect → OAuth URL へ遷移）。
	async function connectGoogle() {
		try {
			const r = await settingsApi.googleOAuthUrl();
			if (r.url) window.location.href = r.url;
		} catch (e) {
			reportError(e);
		}
	}
</script>

<svelte:window onclick={() => (openMenuId = null)} />

<div class="overlay active" id="integrated-overlay">
	<div class="management-overlay-card">
		<div class="management-overlay-header">
			<h1>
				<span class="material-symbols-outlined" style="vertical-align:middle;font-size:1.6rem;"
					>dashboard_customize</span
				> Bot統合管理
			</h1>
			<button type="button" class="btn btn-secondary" onclick={() => navigateTo("/")}>
				<Icon name="arrow_back" class="icon-button-left" /> Bot選択に戻る
			</button>
		</div>
		<div class="management-overlay-body">
<section id="tab-integrated" class="tab-view">
	<p class="description-text int-intro">
		あなたのBotのヘルス確認・起動停止と、認証情報・MCP・Googleアカウントの<strong>登録</strong
		>および<strong>Bot別の利用許可</strong
		>を一括管理します。リソースの登録はこのページから行います（個別タブは状況確認のみ）。
	</p>

	<!-- Bots: health + start/stop -->
	<details class="card collapsible-group int-section" name="int-acc" open>
		<summary>Bot ヘルス / 起動・停止</summary>
		<div class="int-refresh-row">
			<Button variant="secondary" small onclick={loadOverview}>
				<Icon name="refresh" size={16} /> 更新
			</Button>
		</div>
		<div class="int-bots-list">
			{#each bots as b (b.id)}
				{@const chip = botChip(b)}
				<div class="glass int-bot-row">
					<div class="int-bot-info">
						<Icon
							name={b.is_system_default ? "shield_person" : "smart_toy"}
							class="int-bot-icon"
						/>
						<div class="int-bot-meta">
							<div class="int-bot-name">
								{b.name}
								{#if b.is_system_default}<span class="int-bot-sub">(共有秘書)</span
									>{/if}
							</div>
							<div class="int-bot-detail">
								{b.preset}・{b.discord_username || b.id}
							</div>
						</div>
					</div>
					<div class="int-bot-actions">
						<StatusChip status={chip.status} label={chip.label} />
						{#if !b.is_system_default}
							{#if b.suspended || !b.has_token}
								<Button variant="secondary" small disabled
									>{b.running ? "停止" : "起動"}</Button
								>
							{:else if b.running}
								<Button variant="secondary" small onclick={() => botAction("stop", b.id)}
									>停止</Button
								>
							{:else}
								<Button variant="secondary" small onclick={() => botAction("start", b.id)}
									>起動</Button
								>
							{/if}
						{/if}
						<div class="int-menu-wrap">
							<button
								type="button"
								class="int-menu-toggle"
								title="その他の操作"
								aria-label="その他の操作"
								onclick={(e) => {
									e.stopPropagation();
									toggleMenu(b.id);
								}}>⋮</button
							>
							{#if openMenuId === b.id}
								<div class="int-action-menu open" role="menu">
									{#if !b.is_system_default}
										<button
											type="button"
											class="int-menu-item"
											disabled={b.suspended || !b.has_token}
											onclick={(e) => {
												e.stopPropagation();
												botAction("restart", b.id);
											}}>再起動</button
										>
									{/if}
									<button
										type="button"
										class="int-menu-item int-menu-danger"
										title="このBotとの自分の会話履歴をクリア（Redisキャッシュ削除＋境界記録。永続ログは保持）"
										onclick={(e) => {
											e.stopPropagation();
											clearHistory(b.id);
										}}>会話履歴をクリア</button
									>
								</div>
							{/if}
						</div>
					</div>
				</div>
			{/each}
		</div>
	</details>

	<!-- Credentials -->
	<details class="card collapsible-group int-section" name="int-acc">
		<summary>認証情報（パスワードマネージャ）</summary>
		<div class="expense-actions-columns int-cols">
			<div class="action-column">
				<p class="description-text int-col-note">
					新規登録（暗号化保存）。登録後、下の一覧で使わせるBotを許可してください。
				</p>
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
						<input
							type="password"
							id="int-cred-password"
							bind:value={credPassword}
							required
							autocomplete="new-password"
						/>
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
				<p class="description-text int-col-note">登録済み（Bot別の利用許可）</p>
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
	</details>

	<!-- MCP servers -->
	<details class="card collapsible-group int-section" name="int-acc">
		<summary>MCPサーバー</summary>
		<div class="expense-actions-columns int-cols">
			<div class="action-column">
				<p class="description-text int-col-note">
					外部MCPサーバーを登録。登録後、使わせるBotを許可してください。
				</p>
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
						<input
							type="password"
							id="int-mcp-auth"
							bind:value={mcpAuth}
							autocomplete="new-password"
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
							<select id="int-mcp-scope" bind:value={mcpScope}>
								<option value="user">ユーザーレベル（自分のみ）</option>
								<option value="system">システムレベル（全ユーザー）</option>
							</select>
						</div>
					{/if}
					<Button variant="primary" type="submit" block>MCPサーバーを登録</Button>
				</form>
			</div>
			<div class="action-column">
				<p class="description-text int-col-note">登録済み（Bot別の利用許可）</p>
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
										<div class="int-mcp-url">{s.endpoint_url} ・ {s.tools} tools</div>
									</div>
									<div class="int-mcp-actions">
										{#if dashAvailable[s.id]}
											<Button variant="secondary" small onclick={() => openDashboard(s)}
												>管理ページ</Button
											>
										{/if}
										<Button variant="secondary" small onclick={() => toggleMcpServer(s)}
											>{s.enabled ? "無効化" : "有効化"}</Button
										>
										<Button variant="secondary" small onclick={() => deleteMcpServer(s)}
											>削除</Button
										>
									</div>
								</div>
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
	</details>

	<!-- Google accounts -->
	<details class="card collapsible-group int-section" name="int-acc">
		<summary>Googleアカウント連携（複数可）</summary>
		<div class="int-google-body">
			<Button variant="primary" onclick={connectGoogle}>
				<Icon name="add" size={16} /> Googleアカウントを連携
			</Button>
			<p class="description-text int-col-note">
				複数のGoogleアカウントを連携でき、Botごとに使うアカウントを選べます。バックアップはprimaryアカウントを使用します。
			</p>
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

			<p class="description-text int-assign-heading"><strong>Bot別 使用アカウント</strong></p>
			<div class="int-assign-list">
				{#if ownedBots.length === 0}
					<p class="description-text">所有Botがありません。</p>
				{:else}
					{#each ownedBots as b (b.id)}
						<div class="int-assign-row">
							<span class="int-assign-name">{b.name}</span>
							<select
								class="int-assign-select"
								value={assignValue(b)}
								onchange={(e) =>
									onAssignChange(b.id, (e.currentTarget as HTMLSelectElement).value)}
							>
								<option value="primary">（primaryを使用）</option>
								<option value="none">連携なし</option>
								{#each googleAccounts as a (a.id)}
									<option value="acct:{a.id}">{a.email || `アカウント#${a.id}`}</option>
								{/each}
							</select>
						</div>
					{/each}
				{/if}
			</div>
		</div>
	</details>
</section>
		</div>
	</div>
</div>

<IntCalendarsModal bind:open={calOpen} account={calAccount} onsave={saveCalendars} />
<McpDashboardModal bind:open={dashOpen} server={dashServer} />

<style>
	.int-intro {
		margin-bottom: 16px;
	}
	.int-section {
		margin-bottom: 16px;
		padding: 20px;
	}
	.int-refresh-row {
		display: flex;
		justify-content: flex-end;
		margin-top: 12px;
	}
	.int-bots-list {
		display: flex;
		flex-direction: column;
		gap: 10px;
		margin-top: 12px;
	}
	.int-bot-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		padding: 12px 14px;
		border-radius: 8px;
	}
	.int-bot-info {
		display: flex;
		align-items: center;
		gap: 10px;
		min-width: 0;
	}
	.int-bot-meta {
		min-width: 0;
	}
	.int-bot-name {
		font-weight: 600;
	}
	.int-bot-sub {
		font-size: 0.7rem;
		color: var(--color-zinc-muted, #a1a1aa);
	}
	.int-bot-detail {
		font-size: 0.75rem;
		color: var(--color-zinc-muted, #a1a1aa);
	}
	.int-bot-actions {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-shrink: 0;
	}
	.int-menu-wrap {
		position: relative;
	}
	.int-menu-toggle {
		background: none;
		border: 0;
		color: var(--text-secondary, #a1a1aa);
		cursor: pointer;
		font-size: 1.1rem;
		line-height: 1;
		padding: 2px 6px;
	}
	.int-action-menu {
		position: absolute;
		right: 0;
		top: 100%;
		z-index: 20;
		display: flex;
		flex-direction: column;
		min-width: 160px;
		background: var(--surface-2dp, #27272a);
		border: 1px solid var(--border-matte, #333);
		border-radius: 8px;
		padding: 4px;
		box-shadow: 0 6px 18px rgba(0, 0, 0, 0.35);
	}
	.int-menu-item {
		background: none;
		border: 0;
		color: var(--text-high, #fff);
		text-align: left;
		padding: 8px 10px;
		border-radius: 6px;
		cursor: pointer;
		font-size: 0.85rem;
	}
	.int-menu-item:hover:not(:disabled) {
		background: var(--surface-1dp, rgba(255, 255, 255, 0.06));
	}
	.int-menu-item:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}
	.int-menu-danger {
		color: #ef4444;
	}
	.int-cols {
		margin-top: 12px;
	}
	.int-col-note {
		margin-bottom: 8px;
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
	.int-assign-select {
		min-width: 200px;
	}
</style>
