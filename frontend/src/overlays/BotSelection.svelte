<script lang="ts">
	// ─────────────────────────────────────────────────────────────────────────
	// BotSelection — Bot 一覧・作成・切替（管理ポータル）。
	// 旧 index.html #bot-selection-overlay + app.js の fetchBotList / renderBotList /
	// selectBot / createBotForm / btn-open-* 配線を移植。
	//
	// - 一覧取得は botApi.list()（scope:'user'）。統計カードは connected/running で集計。
	// - Bot 選択は activeBot ストア（selectBot）へ委譲 → /bot/config へ遷移。
	// - 作成は共通 Modal。プリセット表示名は botApi.presets() で上書き。
	// - Discord 同期 / プロフィール編集 / 削除（デフォルト Bot 以外）。
	// - 統合管理 / アカウント管理 / 管理者設定（admin のみ）への遷移。
	// ─────────────────────────────────────────────────────────────────────────
	import { onMount } from "svelte";
	import { botApi } from "$lib/api/services";
	import { ApiError } from "$lib/api/client";
	import { pushToast } from "$lib/stores/toast";
	import { AdminPageHeader, confirmDialog, Modal, Icon } from "$lib/components/ui";
	import { selectBot } from "$lib/stores/activeBot";
	import { currentUser, isAdmin } from "$lib/stores/session";
	import { theme, toggleTheme } from "$lib/stores/theme";
	import { authApi } from "$lib/api/services";
	import { navigateTo } from "$lib/router";
	import type { BotView, PresetOption } from "$lib/api/types";

	let bots = $state<BotView[]>([]);
	let presets = $state<PresetOption[]>([]);
	let loading = $state(false);

	// 作成モーダル
	let createOpen = $state(false);
	let newBotName = $state("");
	let newBotPreset = $state("secretary");

	// プロフィール編集モーダル
	let editOpen = $state(false);
	let editBotId = $state("");
	let editBotName = $state("");
	let editBotAvatar = $state("");

	function reportError(e: unknown): void {
		pushToast(e instanceof ApiError ? e.message : "エラーが発生しました", "error");
	}

	function isDefaultBot(id: string): boolean {
		return id.startsWith("bot_default_") || id === "system_default";
	}

	async function loadBots(): Promise<void> {
		loading = true;
		try {
			const res = await botApi.list();
			bots = res.bots ?? [];
		} catch (e) {
			reportError(e);
			bots = [];
		} finally {
			loading = false;
		}
	}

	async function loadPresets(): Promise<void> {
		try {
			const res = await botApi.presets();
			presets = res.presets ?? [];
		} catch {
			/* 表示名は既定値で続行 */
		}
	}

	onMount(() => {
		void loadBots();
		void loadPresets();
	});

	// ── 統計（旧 renderBotList の home-stats） ──
	const total = $derived(bots.length);
	const online = $derived(bots.filter((b) => b.connected).length);
	const connecting = $derived(bots.filter((b) => b.running && !b.connected).length);
	const stopped = $derived(bots.filter((b) => !b.running).length);

	// ── プリセット表示名（作成 select 用ラベル。旧 refreshPresetOptions） ──
	function presetLabel(preset: string, fallback: string): string {
		const p = presets.find((x) => x.preset === preset);
		return p?.display_name ?? fallback;
	}

	// ── Bot カードの表示メタ（旧 renderBotList のステータス計算） ──
	function statusOf(bot: BotView): { dot: string; label: string } {
		if (bot.suspended) return { dot: "dot-offline", label: "停止（管理者）" };
		if (bot.connected)
			return { dot: "dot-online", label: bot.shared ? "共有Botで稼働" : "稼働中" };
		if (bot.running) return { dot: "dot-connecting", label: "接続中…" };
		return { dot: "dot-offline", label: "停止中" };
	}

	function displayName(bot: BotView): string {
		return bot.discord_username || bot.name;
	}

	// ── Bot 選択（旧 selectBot） ──
	function choose(bot: BotView): void {
		selectBot({
			id: bot.id,
			name: displayName(bot),
			avatar: bot.discord_avatar_url || "",
			preset: bot.preset || "secretary",
		});
		// 2階層化後の入口は一般情報（設定系はサイドバー「Bot設定」ハブ配下へ移動）。
		navigateTo("/bot/dashboard");
	}

	// ── Discord 同期（旧 syncBtn） ──
	async function syncDiscord(bot: BotView): Promise<void> {
		try {
			await botApi.syncDiscord(bot.id);
			await loadBots();
		} catch (e) {
			reportError(e);
		}
	}

	// ── プロフィール編集モーダル ──
	function openEdit(bot: BotView): void {
		editBotId = bot.id;
		editBotName = displayName(bot);
		editBotAvatar = bot.discord_avatar_url || "";
		editOpen = true;
	}

	async function saveEdit(e: SubmitEvent): Promise<void> {
		e.preventDefault();
		try {
			await botApi.updateProfile({
				botId: editBotId,
				name: editBotName.trim(),
				avatarUrl: editBotAvatar.trim() || null,
			});
			editOpen = false;
			await loadBots();
		} catch (e) {
			reportError(e);
		}
	}

	// ── 削除（旧 delBtn。confirm → confirmDialog） ──
	async function removeBot(bot: BotView): Promise<void> {
		const ok = await confirmDialog({
			message: `本当にBot「${bot.name}」を削除しますか？\n紐づく経費やタスクデータも全て削除されます。`,
			danger: true,
			confirmLabel: "削除",
		});
		if (!ok) return;
		try {
			await botApi.remove(bot.id);
			await loadBots();
		} catch (e) {
			reportError(e);
		}
	}

	// ── 作成（旧 createBotForm submit） ──
	function openCreate(): void {
		newBotName = "";
		newBotPreset = "secretary";
		createOpen = true;
	}

	async function submitCreate(e: SubmitEvent): Promise<void> {
		e.preventDefault();
		try {
			const res = await botApi.create({
				name: newBotName.trim(),
				preset: newBotPreset,
			});
			createOpen = false;
			if (newBotPreset === "mcp_assistant" && res.message) {
				pushToast(res.message, "info", 8000);
			}
			await loadBots();
		} catch (e) {
			reportError(e);
		}
	}

	// ── ログアウト（旧 btnBotLogout） ──
	async function logout(): Promise<void> {
		try {
			await authApi.logout();
		} catch {
			/* ローカル破棄を優先 */
		}
		selectBot(null);
		currentUser.set(null);
		navigateTo("/login");
	}

	// スマホ用ドロワー開閉（BotShell と同規約。PC はサイドバー常設）。
	let sidebarOpen = $state(false);

	// サイドバー下部ユーザーパネル表示（BotShell と同一フォーマット）。
	const userDisplay = $derived(
		$currentUser ? `${$currentUser.username} (${$currentUser.discordId})` : "ロード中...",
	);
	const themeIcon = $derived($theme === "dark" ? "light_mode" : "dark_mode");
	const themeTitle = $derived(
		$theme === "dark" ? "ライトテーマに切り替え" : "ダークテーマに切り替え",
	);
</script>

<svelte:window
	onkeydown={(e) => {
		if (e.key === "Escape") sidebarOpen = false;
	}}
/>

<div class="app-container">
	<!-- サイドバー（PC 常設 / スマホはドロワー。BotShell と同じ構造・クラスを流用） -->
	<aside class="sidebar" class:open={sidebarOpen}>
		<div class="home-nav-brand home-sidebar-brand">
			<span class="material-symbols-outlined home-nav-logo">calculate</span>
			<span class="home-nav-title">Yuuka</span>
		</div>

		<nav class="sidebar-menu">
			<button
				type="button"
				class="menu-item active"
				onclick={() => (sidebarOpen = false)}
			>
				<Icon name="home" class="menu-icon-symbol" />
				<span class="menu-text">ホーム（Bot一覧）</span>
			</button>
			<button
				type="button"
				class="menu-item"
				onclick={() => navigateTo("/integrated")}
			>
				<Icon name="hub" class="menu-icon-symbol" />
				<span class="menu-text">統合管理</span>
			</button>
			<button
				type="button"
				class="menu-item"
				onclick={() => navigateTo("/account")}
			>
				<Icon name="manage_accounts" class="menu-icon-symbol" />
				<span class="menu-text">アカウント管理</span>
			</button>
			{#if $isAdmin}
				<button
					type="button"
					class="menu-item"
					onclick={() => navigateTo("/admin")}
				>
					<Icon name="admin_panel_settings" class="menu-icon-symbol" />
					<span class="menu-text">管理者設定</span>
				</button>
			{/if}
		</nav>

		<!-- サイドバー最下部ユーザーパネル（BotShell と共通クラス） -->
		<div class="sidebar-user-panel">
			<div class="sidebar-user-info" title={userDisplay}>
				<Icon name="person" class="icon-small" />
				<span class="sidebar-user-name">{userDisplay}</span>
			</div>
			<div class="sidebar-user-actions">
				<button
					type="button"
					class="btn-icon"
					title={themeTitle}
					aria-label={themeTitle}
					onclick={toggleTheme}
				>
					<Icon name={themeIcon} />
				</button>
				<button
					type="button"
					class="btn-icon"
					title="ログアウト"
					aria-label="ログアウト"
					onclick={logout}
				>
					<Icon name="logout" />
				</button>
			</div>
		</div>
	</aside>

	<!-- ドロワー背面オーバーレイ（スマホのみ） -->
	{#if sidebarOpen}
		<button
			type="button"
			class="sidebar-backdrop"
			aria-label="メニューを閉じる"
			onclick={() => (sidebarOpen = false)}
		></button>
	{/if}

	<main class="main-content">
		<AdminPageHeader class="bot-selection-header">
			<h2>Bot一覧</h2>
		</AdminPageHeader>

		<div class="home-dashboard">

		<!-- グリーティング + サマリー -->
		<div class="home-hero">
			<div class="home-greeting">
				<h1 class="home-greeting-title">
					おかえりなさい、{$currentUser?.username ?? "—"}
				</h1>
				<p class="home-greeting-sub">AIアシスタントの管理ポータルへようこそ</p>
			</div>
			<div class="home-stats">
				<div class="home-stat-card">
					<div class="home-stat-label">TOTAL BOTS</div>
					<div class="home-stat-value">{total}</div>
					<div class="home-stat-sub">登録済みアシスタント</div>
				</div>
				<div class="home-stat-card stat-online">
					<div class="home-stat-label">ONLINE</div>
					<div class="home-stat-value">{online}</div>
					<div class="home-stat-sub">Discord接続中</div>
				</div>
				<div class="home-stat-card" class:stat-warning={connecting > 0}>
					<div class="home-stat-label">CONNECTING</div>
					<div class="home-stat-value">{connecting}</div>
					<div class="home-stat-sub">接続処理中</div>
				</div>
				<div class="home-stat-card">
					<div class="home-stat-label">STOPPED</div>
					<div class="home-stat-value">{stopped}</div>
					<div class="home-stat-sub">停止中</div>
				</div>
			</div>
		</div>

		<!-- Bot セクション -->
		<div class="home-section">
			<div class="home-section-header">
				<div class="home-section-label">
					<span class="material-symbols-outlined home-section-icon">robot_2</span>
					<h2 class="home-section-title">アシスタント Bot</h2>
				</div>
				<button type="button" class="btn btn-primary btn-sm" onclick={openCreate}>
					<Icon name="add" class="icon-button-left" />新規作成
				</button>
			</div>

			<div class="bot-cards-grid">
				{#each bots as bot (bot.id)}
					{@const st = statusOf(bot)}
					<div
						class="bot-item-card"
						class:default-bot={isDefaultBot(bot.id)}
						role="button"
						tabindex="0"
						onclick={() => choose(bot)}
						onkeydown={(e) => {
							if (e.key === "Enter" || e.key === " ") {
								e.preventDefault();
								choose(bot);
							}
						}}
					>
						<div class="bot-card-top">
							<div class="bot-avatar">
								{#if bot.discord_avatar_url}
									<img src={bot.discord_avatar_url} alt={displayName(bot)} />
								{:else}
									<span class="material-symbols-outlined" style="font-size: 22px;">robot_2</span>
								{/if}
							</div>
							<div class="bot-info">
								<div class="bot-name" title={displayName(bot)}>{displayName(bot)}</div>
								<div class="bot-status">
									{bot.preset_display_name ||
										(bot.preset === "mcp_assistant" ? "汎用モード" : "パーソナル秘書")}
								</div>
							</div>
						</div>
						<div class="bot-card-footer">
							<div class="bot-run-status">
								<span class="bot-run-dot {st.dot}"></span>{st.label}
							</div>
							<div class="bot-item-actions">
								<button
									type="button"
									class="btn-sync-discord"
									title="Discordから名前・アイコンを同期"
									onclick={(e) => {
										e.stopPropagation();
										void syncDiscord(bot);
									}}
								>
									<Icon name="sync" size={15} />
								</button>
								{#if !isDefaultBot(bot.id)}
									<button
										type="button"
										class="btn-sync-discord"
										title="名前・アイコンを編集"
										onclick={(e) => {
											e.stopPropagation();
											openEdit(bot);
										}}
									>
										<Icon name="edit" size={15} />
									</button>
									<button
										type="button"
										class="btn-delete-bot-inline"
										title="Botを削除"
										onclick={(e) => {
											e.stopPropagation();
											void removeBot(bot);
										}}
									>
										<Icon name="delete" size={15} />
									</button>
								{/if}
							</div>
						</div>
					</div>
				{/each}
			</div>
		</div>
		</div>
	</main>
</div>

<style>
	/* BotShell と同じ骨格（scoped のため各ページで宣言が必要） */
	.app-container {
		display: flex;
		min-height: 100vh;
	}
	.menu-item {
		background: none;
		border: none;
		width: 100%;
		cursor: pointer;
		font: inherit;
		text-align: left;
	}
	/* サイドバー内ブランド行 */
	.home-sidebar-brand {
		margin: 4px 4px 20px;
	}

	/* main-content の余白は一覧本文にだけ適用し、共通ヘッダーは画面端まで広げる。 */
	:global(.bot-selection-header) {
		margin: -40px -40px 28px;
	}

	@media (max-width: 1024px) and (min-width: 769px) {
		:global(.bot-selection-header) {
			margin: -28px -24px 24px;
		}
	}

	@media (max-width: 768px) {
		:global(.bot-selection-header) {
			margin: -18px -14px 18px;
		}
	}

	@media (max-width: 420px) {
		:global(.bot-selection-header) {
			margin: -16px -12px 16px;
		}
	}

	@media (max-width: 360px) {
		:global(.bot-selection-header) {
			margin: -14px -10px 14px;
		}
	}
</style>

<!-- Bot 作成モーダル -->
<Modal bind:open={createOpen} title="新規Botの作成">
	<form onsubmit={submitCreate}>
		<div class="form-group">
			<label for="new-bot-name">Botの名前 *</label>
			<input
				type="text"
				id="new-bot-name"
				required
				placeholder="例: アシスタント"
				bind:value={newBotName}
			/>
			<span class="field-sub">※AIのキャラクター設定は作成後に「ペルソナ」タブで管理できます。</span>
		</div>
		<div class="form-group">
			<label for="new-bot-preset">Botの種類（プリセット）</label>
			<select id="new-bot-preset" bind:value={newBotPreset}>
				<option value="secretary">{presetLabel("secretary", "パーソナル秘書")}</option>
				<option value="mcp_assistant">{presetLabel("mcp_assistant", "汎用モード")}</option>
			</select>
			<span class="field-sub"
				>※パーソナル秘書: 現行のフル機能（個人向け）。汎用モード: MCP接続・ペルソナ・メモリのみのサーバー常駐Bot（Bot専用のGemini
				APIキーが必要）。</span
			>
		</div>
		<button type="submit" class="btn btn-primary btn-block">作成する</button>
	</form>
</Modal>

<!-- Bot プロフィール編集モーダル -->
<Modal bind:open={editOpen} title="Botプロフィールの編集">
	<form onsubmit={saveEdit}>
		<div class="form-group">
			<label for="edit-bot-profile-name">表示名 *</label>
			<input type="text" id="edit-bot-profile-name" required bind:value={editBotName} />
		</div>
		<div class="form-group">
			<label for="edit-bot-profile-avatar">アバターURL</label>
			<input
				type="text"
				id="edit-bot-profile-avatar"
				placeholder="https://..."
				bind:value={editBotAvatar}
			/>
		</div>
		<button type="submit" class="btn btn-primary btn-block">保存する</button>
	</form>
</Modal>
