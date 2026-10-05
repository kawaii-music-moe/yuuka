<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// HomeSidebar — 管理ポータル（Bot 一覧 / 統合管理 / アカウント管理 / 管理者設定）
// 共通の左サイドバー。BotSelection と ManagementOverlayShell の両方から使い、
// どの画面からでも同じ導線で行き来できるようにする。
//
// - 選択中の項目は currentRoute から判定（呼び出し側で指定不要）。
// - PC は常設、スマホはドロワー（open を親と bind して ☰ で開閉。BotShell と同規約）。
// ─────────────────────────────────────────────────────────────────────────

import { authApi } from "$lib/api/services";
import { Icon } from "$lib/components/ui";
import { currentRoute, navigateTo } from "$lib/router";
import { selectBot } from "$lib/stores/activeBot";
import { currentUser, isAdmin } from "$lib/stores/session";
import { theme, toggleTheme } from "$lib/stores/theme";

interface Props {
	/** スマホ用ドロワーの開閉状態 */
	open?: boolean;
}
let { open = $bindable(false) }: Props = $props();

const items = $derived([
	{ path: "/", icon: "home", label: "ホーム（Bot一覧）" },
	{ path: "/integrated", icon: "hub", label: "統合管理" },
	{ path: "/account", icon: "manage_accounts", label: "アカウント管理" },
	...($isAdmin
		? [{ path: "/admin", icon: "admin_panel_settings", label: "管理者設定" }]
		: []),
]);

function isActive(path: string): boolean {
	if (path === "/")
		return !items.some(
			(i) => i.path !== "/" && $currentRoute.startsWith(i.path),
		);
	return $currentRoute.startsWith(path);
}

function go(path: string): void {
	open = false;
	if ($currentRoute !== path) navigateTo(path);
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

// サイドバー下部ユーザーパネル表示（BotShell と同一フォーマット）。
const userDisplay = $derived(
	$currentUser
		? `${$currentUser.username} (${$currentUser.discordId})`
		: "ロード中...",
);
const themeIcon = $derived($theme === "dark" ? "light_mode" : "dark_mode");
const themeTitle = $derived(
	$theme === "dark" ? "ライトテーマに切り替え" : "ダークテーマに切り替え",
);
</script>

<svelte:window
	onkeydown={(e) => {
		if (e.key === "Escape") open = false;
	}}
/>

<!-- サイドバー（PC 常設 / スマホはドロワー。BotShell と同じ構造・クラスを流用） -->
<aside class="sidebar" class:open>
	<div class="home-nav-brand home-sidebar-brand">
		<span class="material-symbols-outlined home-nav-logo">calculate</span>
		<span class="home-nav-title">Yuuka</span>
	</div>

	<nav class="sidebar-menu">
		{#each items as item (item.path)}
			<button
				type="button"
				class="menu-item"
				class:active={isActive(item.path)}
				aria-current={isActive(item.path) ? "page" : undefined}
				onclick={() => go(item.path)}
			>
				<Icon name={item.icon} class="menu-icon-symbol" />
				<span class="menu-text">{item.label}</span>
			</button>
		{/each}
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
{#if open}
	<button
		type="button"
		class="sidebar-backdrop"
		aria-label="メニューを閉じる"
		onclick={() => (open = false)}
	></button>
{/if}

<style>
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
</style>
