<script lang="ts">
	// ─────────────────────────────────────────────────────────────────────────
	// BotShell — /bot/<tab> の骨格シェル（旧 index.html #app-container +
	// app.js の switchTab / サイドバー配線 / ヘッダ操作を移植）。
	//
	// 重要（統合契約）: 「タブ名 → Bot*.svelte の対応・動的コンポーネントスイッチ」は
	//   完全に維持する（§P1b で静的 import を遅延 loader マップに変換したが、
	//   タブ名の対応関係とスイッチの外部契約は不変）。後続タブ担当は
	//   routes/Bot*.svelte だけ差し替えればよい。
	//
	// 実装するもの:
	//   - サイドバー（Bot一覧へ戻る / Bot ブランディング / プリセット別メニュー・active）
	//   - ヘッダ（現タブタイトル・Bot切替バッジ・ユーザーバッジ・テーマトグル・ログアウト）
	//   - タブ切替は navigateTo('/bot/<tab>')。表示中タブは props.tab を受ける。
	//   - 秘書/汎用プリセット別タブフィルタ（SECRETARY_ONLY_TABS）+ 設定ハブ
	//     2階層化（設定系タブは $lib/botTabs を単一情報源に /bot/settings 配下）。
	// ─────────────────────────────────────────────────────────────────────────
	import type { Component } from "svelte";
	import { derived } from "svelte/store";
	import { activeBot, selectBot } from "$lib/stores/activeBot";
	import { currentUser } from "$lib/stores/session";
	import { theme, toggleTheme } from "$lib/stores/theme";
	import { authApi } from "$lib/api/services";
	import { pushToast } from "$lib/stores/toast";
	import { navigateTo, DEFAULT_BOT_TAB, type BotTab } from "$lib/router";
	import { SETTINGS_CHILD_TABS, botPreset } from "$lib/botTabs";
	import { Icon } from "$lib/components/ui";

	// §P1b ルート遅延ロード: 16タブの静的 import を loader マップに変換する。
	// Vite は各 import() を個別チャンク（BotDashboard-*.js 等）に分割するため、
	// 初期 entry（index-*.js）からタブ実体が外れ初期JSが軽くなる。
	// 各タブは props 形状が異なり得るため Component<Record<string, never>> で受ける。
	type Loader = () => Promise<{ default: Component<Record<string, never>> }>;

	// §8 タブ → loader 完全網羅マップ（統合契約: キー集合は不変に保つ）。
	const TAB_LOADERS: Record<BotTab, Loader> = {
		dashboard: () => import("./BotDashboard.svelte"),
		tasks: () => import("./BotTasks.svelte"),
		timeline: () => import("./BotTimeline.svelte"),
		schedules: () => import("./BotSchedules.svelte"),
		expenses: () => import("./BotExpenses.svelte"),
		reminders: () => import("./BotReminders.svelte"),
		personal: () => import("./BotPersonal.svelte"),
		personas: () => import("./BotPersonas.svelte"),
		delivery: () => import("./BotDelivery.svelte"),
		webhooks: () => import("./BotWebhooks.svelte"),
		mcp: () => import("./BotMcp.svelte"),
		playbooks: () => import("./BotPlaybooks.svelte"),
		discord: () => import("./BotDiscord.svelte"),
		config: () => import("./BotConfig.svelte"),
		devices: () => import("./BotDevices.svelte"),
		settings: () => import("./BotSettingsHub.svelte"),
	};

	// 現在表示中のタブ（App/router から渡される。未指定は既定 DEFAULT_BOT_TAB）。
	let { tab = DEFAULT_BOT_TAB }: { tab?: BotTab } = $props();

	// §8 プリセット別タブフィルタ（旧 app.js:157-170）。
	// 2階層化: 設定系（personas/delivery/webhooks/mcp/playbooks/discord/config/
	// devices）はサイドバーから外し /bot/settings ハブへ集約（BotSettingsHub）。
	const SECRETARY_ONLY_TABS: BotTab[] = [
		"tasks",
		"timeline",
		"schedules",
		"expenses",
		"reminders",
		"personal",
	];

	// サイドバー項目（日常機能のみ + 設定ハブ）。
	// 設定ハブ配下のタブ集合は $lib/botTabs の SETTINGS_CHILD_TABS（単一情報源）。
	const ALL_MENU: { tab: BotTab; label: string; icon: string }[] = [
		{ tab: "dashboard", label: "一般情報", icon: "info" },
		{ tab: "tasks", label: "タスク管理", icon: "checklist" },
		{ tab: "schedules", label: "予定スケジュール", icon: "calendar_today" },
		{ tab: "reminders", label: "リマインダー", icon: "alarm" },
		{ tab: "timeline", label: "タイムライン", icon: "timeline" },
		{ tab: "expenses", label: "経費・収支管理", icon: "payments" },
		{ tab: "personal", label: "メモ・連絡先", icon: "contacts" },
		{ tab: "settings", label: "Bot設定", icon: "settings" },
	];

	// §8 ヘッダタイトルマップ（旧 app.js:338-354 switchTab）。
	const TAB_TITLES: Record<BotTab, string> = {
		dashboard: "ダッシュボード",
		tasks: "タスク管理",
		timeline: "デイリータイムライン",
		schedules: "予定スケジュール",
		expenses: "家計管理",
		reminders: "リマインダー",
		personal: "メモ・連絡先",
		personas: "ペルソナ管理",
		delivery: "配信設定",
		webhooks: "Webhook 連携",
		mcp: "MCPサーバー管理",
		playbooks: "Playbook 設定",
		discord: "Discord 連携設定",
		config: "システム設定情報",
		devices: "接続端末",
		settings: "Bot設定",
	};

	// プリセット別に絞り込んだメニュー配列（derived(activeBot)）。
	// 設定系タブの表示条件は BotSettingsHub が $lib/botTabs の同じ規約で絞り込む。
	const menuItems = derived(activeBot, ($bot) => {
		const isAssistant = botPreset($bot) === "mcp_assistant";
		return ALL_MENU.filter((m) => {
			if (SECRETARY_ONLY_TABS.includes(m.tab)) return !isAssistant;
			return true;
		});
	});

	// スマホ用ドロワー開閉（≤768px。PC ではサイドバー常設のため未使用）。
	let sidebarOpen = $state(false);

	function go(t: BotTab): void {
		sidebarOpen = false; // 遷移時はドロワーを閉じる
		navigateTo(`/bot/${t}`);
	}

	// Bot切替（旧 btn-switch-bot / sidebar-btn-back-bots → navigateTo("/")）。
	function backToBots(): void {
		navigateTo("/");
	}

	// ログアウト（旧 btnLogout: /api/logout → Bot 選択解除 → /login）。
	async function logout(): Promise<void> {
		try {
			await authApi.logout();
		} catch {
			/* ログアウトはローカル状態の破棄を優先。通信失敗でも続行 */
		}
		selectBot(null);
		currentUser.set(null);
		navigateTo("/login");
	}

	// 現タブに対応するチャンクの Promise（未知タブは config フォールバック）。
	// tab をキーに $derived で一度だけ Promise を生成することで、tab 変更時のみ
	// import() が再評価される。同一タブ内の他の再レンダリングでは同じ Promise 参照の
	// ままなので無駄なフェッチが起きない（Vite の module cache で実 fetch も1回）。
	const modulePromise = $derived(
		(TAB_LOADERS[tab] ?? TAB_LOADERS[DEFAULT_BOT_TAB])(),
	);
	// 設定ハブ配下ページか（パンくず表記とサイドバー「Bot設定」アクティブ判定に使用）。
	const inSettingsChild = $derived(SETTINGS_CHILD_TABS.includes(tab));
	const title = $derived(
		inSettingsChild
			? `Bot設定 › ${TAB_TITLES[tab]}`
			: (TAB_TITLES[tab] ?? "ダッシュボード"),
	);

	// Bot ブランディング（旧 updateSidebarBotBranding）。
	const botName = $derived($activeBot?.name ?? "システムデフォルト");
	const botId = $derived($activeBot?.id ?? "system_default");
	const botIdLabel = $derived(botId.startsWith("bot_") ? botId.slice(4) : botId);
	const botAvatar = $derived($activeBot?.avatar ?? "");
	const DEFAULT_AVATAR =
		"https://assets-global.website-files.com/6257adef93867e50d84d30e2/636e0a6a49cf127bf92de1e2_icon_clyde_blurple_RGB.png";

	// ヘッダのユーザー表示（旧 current-user-display: "username (discordId)"）。
	const userDisplay = $derived(
		$currentUser ? `${$currentUser.username} (${$currentUser.discordId})` : "ロード中...",
	);

	// テーマトグルのアイコン/ツールチップ（旧 applyThemeIcon）。
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

<div class="app-container" id="app-container">
	<aside class="sidebar" class:open={sidebarOpen}>
		<button
			type="button"
			class="sidebar-back-button"
			title="Bot一覧に戻る"
			onclick={backToBots}
		>
			<Icon name="chevron_left" />
			<span>アプリケーション一覧</span>
		</button>

		<div class="app-sidebar-header">
			<div class="app-sidebar-avatar-container">
				<img src={botAvatar || DEFAULT_AVATAR} alt="App Icon" />
			</div>
			<div class="app-sidebar-meta">
				<h3>{botName}</h3>
				<span class="app-id-label">APPLICATION ID</span>
				<span class="app-id-value">{botIdLabel}</span>
			</div>
		</div>

		<nav class="sidebar-menu">
			{#each $menuItems as item (item.tab)}
				<button
					type="button"
					class="menu-item"
					class:active={item.tab === tab ||
						(item.tab === "settings" && inSettingsChild)}
					class:menu-item-settings={item.tab === "settings"}
					data-tab={item.tab}
					onclick={() => go(item.tab)}
				>
					<Icon name={item.icon} class="menu-icon-symbol" />
					<span class="menu-text">{item.label}</span>
				</button>
			{/each}
		</nav>

		<!-- サイドバー最下部ユーザーパネル（テーマ切替/ログアウト集約。ヘッダーから移設） -->
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

	<!-- ドロワー背面オーバーレイ（スマホのみ。タップで閉じる） -->
	{#if sidebarOpen}
		<button
			type="button"
			class="sidebar-backdrop"
			aria-label="メニューを閉じる"
			onclick={() => (sidebarOpen = false)}
		></button>
	{/if}

	<main class="main-content">
		<header class="top-header">
			<button
				type="button"
				class="menu-toggle"
				aria-label="メニューを開く"
				onclick={() => (sidebarOpen = true)}
			>
				<Icon name="menu" />
			</button>
			<div class="header-title">
				<h2 id="current-tab-title">{title}</h2>
			</div>
		</header>

		<div class="content-view-container">
			{#await modulePromise}
				<div class="tab-loading" aria-busy="true"></div>
			{:then module}
				{@const TabView = module.default}
				<TabView />
			{:catch}
				<div class="tab-error" role="alert">
					タブの読み込みに失敗しました。再読込してください。
				</div>
			{/await}
		</div>
	</main>
</div>

<style>
	/* 旧 #app-container の id セレクタ由来レイアウトを維持しつつ、
	   Svelte スコープでも同じ flex レイアウトを保証する。 */
	.app-container {
		display: flex;
		min-height: 100vh;
	}
	.sidebar-back-button {
		background: none;
		border: none;
		cursor: pointer;
		width: 100%;
		text-align: left;
	}
	/* スマホもドロワー（縦型サイドバー）のため PC と同じ全幅・左寄せでよい */
	.menu-item {
		background: none;
		border: none;
		width: 100%;
		cursor: pointer;
		font: inherit;
		text-align: left;
	}
	/* 設定ハブ項目は日常機能と視覚的に区切る（2階層化） */
	.menu-item-settings {
		margin-top: 10px;
		border-top: 1px solid var(--border-divider);
		border-radius: 0;
		padding-top: 14px;
	}
	.content-view-container {
		flex: 1;
	}
	.tab-loading {
		flex: 1;
		min-height: 60vh;
	}
	.tab-error {
		flex: 1;
		padding: 2rem;
		text-align: center;
	}
</style>
