<script lang="ts">
	import { authApi } from "$lib/api/services";
	import { Icon } from "$lib/components/ui";
	import { navigateTo } from "$lib/router";
	import { activeBot, selectBot } from "$lib/stores/activeBot";
	import { closeAdminNavigation, adminNavigationOpen } from "$lib/stores/adminNavigation";
	import { currentUser, isAdmin } from "$lib/stores/session";

	const items = [
		{ path: "/", icon: "home", label: "Bot一覧" },
		{ path: "/integrated", icon: "hub", label: "統合管理" },
		{ path: "/account", icon: "manage_accounts", label: "アカウント管理" },
	];

	function go(path: string): void {
		closeAdminNavigation();
		navigateTo(path);
	}

	async function logout(): Promise<void> {
		try { await authApi.logout(); } catch { /* ローカル状態の破棄を優先 */ }
		selectBot(null);
		currentUser.set(null);
		closeAdminNavigation();
		navigateTo("/login");
	}

	const currentBot = $derived($activeBot?.name ?? "Bot未選択");
</script>

{#if $adminNavigationOpen}
	<button class="admin-drawer-backdrop" type="button" aria-label="メニューを閉じる" onclick={closeAdminNavigation}></button>
	<aside class="admin-navigation-drawer" aria-label="管理メニュー">
		<div class="admin-drawer-brand">
			<Icon name="smart_toy" />
			<span>Yuuka</span>
			<button type="button" class="admin-drawer-close" aria-label="メニューを閉じる" onclick={closeAdminNavigation}><Icon name="close" /></button>
		</div>
		<nav class="admin-drawer-nav">
			{#each items as item (item.path)}
				<button type="button" onclick={() => go(item.path)}><Icon name={item.icon} /><span>{item.label}</span></button>
			{/each}
			{#if $isAdmin}
				<button type="button" onclick={() => go("/admin")}><Icon name="admin_panel_settings" /><span>管理者設定</span></button>
			{/if}
			{#if $activeBot}
				<button type="button" onclick={() => go("/bot/dashboard")}><Icon name="space_dashboard" /><span>{currentBot}</span></button>
			{/if}
		</nav>
		<div class="admin-drawer-footer">
			<div class="admin-drawer-user"><Icon name="account_circle" /><span>{$currentUser?.username ?? "ユーザー"}</span></div>
			<button type="button" class="admin-drawer-logout" onclick={logout}><Icon name="logout" /><span>ログアウト</span></button>
		</div>
	</aside>
{/if}

<style>
	.admin-drawer-backdrop { position: fixed; z-index: 10009; inset: 0; border: 0; background: rgba(0, 0, 0, 0.58); }
	.admin-navigation-drawer { position: fixed; z-index: 10010; inset: 0 auto 0 0; width: min(320px, 88vw); display: flex; flex-direction: column; background: var(--surface-1dp); border-right: 1px solid var(--border-divider); box-shadow: 12px 0 32px rgba(0, 0, 0, 0.34); }
	.admin-drawer-brand { min-height: 58px; padding: 0 16px; display: flex; align-items: center; gap: 8px; border-bottom: 1px solid var(--border-divider); color: var(--color-primary); font-weight: 700; }
	.admin-drawer-close { width: 36px; height: 36px; margin-left: auto; display: grid; place-items: center; border: 0; border-radius: 4px; background: transparent; color: inherit; cursor: pointer; }
	.admin-drawer-close:hover, .admin-drawer-nav button:hover, .admin-drawer-logout:hover { background: rgba(21, 94, 239, 0.14); color: var(--text-primary); }
	.admin-drawer-nav { display: grid; gap: 2px; padding: 10px 8px; overflow: auto; }
	.admin-drawer-nav button, .admin-drawer-user, .admin-drawer-logout { min-height: 44px; display: flex; align-items: center; gap: 12px; padding: 0 12px; border: 0; border-radius: 4px; background: transparent; color: var(--text-secondary); font: inherit; text-align: left; }
	.admin-drawer-nav button { cursor: pointer; }
	.admin-drawer-nav button span, .admin-drawer-user span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.admin-drawer-footer { margin-top: auto; display: grid; gap: 2px; padding: 8px; border-top: 1px solid var(--border-divider); }
	.admin-drawer-user { font-size: 0.82rem; }
	.admin-drawer-logout { width: 100%; cursor: pointer; }
</style>
