<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// ManagementOverlayShell — 全体管理ページ共通シェル。
// アカウント管理 / Bot統合管理 / 管理者設定の3画面の骨格を集約する。
// 本文幅はホーム（BotSelection）と同じ .home-dashboard に揃え、
// 左サイドバーは共通の HomeSidebar（ホームへの導線もここ）。
//
// - ハブ（/<base>）: hubHeader（導入文など）+ セクションの縦並びリスト（罫線区切り）。
// - 個別（/<base>/<key>）: パンくず（<title> › <セクション名>）+ children(key)。
// 見出しは BotShell と同じ .top-header / .header-title / .breadcrumb-link で揃える。
// セクション定義は $lib/managementSections が単一情報源。
// styles.css の #id 別規則（#integrated-overlay / #admin-overlay）が効くよう id は props で受ける。
// ─────────────────────────────────────────────────────────────────────────
import type { Snippet } from "svelte";
import { Icon } from "$lib/components/ui";
import {
	type ManagementSection,
	sectionFromRoute,
} from "$lib/managementSections";
import { currentRoute, navigateTo } from "$lib/router";
import HomeSidebar from "./HomeSidebar.svelte";

interface Props {
	/** ルート要素 id（CSS が参照） */
	id: string;
	title: string;
	/** ハブのパス（例: "/account"）。個別ページは `${base}/${key}` */
	base: string;
	sections: ManagementSection[];
	/** ハブのリスト上部に出す内容（導入文・KPI 等・任意） */
	hubHeader?: Snippet;
	/** 選択中セクションの中身（引数はセクションキー） */
	children: Snippet<[string]>;
}
let { id, title, base, sections, hubHeader, children }: Props = $props();

const current = $derived(sectionFromRoute($currentRoute, base, sections));

// スマホ用ドロワー開閉（PC はサイドバー常設）。
let sidebarOpen = $state(false);
</script>

<div class="app-container" {id}>
	<HomeSidebar bind:open={sidebarOpen} />

	<main class="main-content">
		<div class="home-dashboard">
			<!-- 見出し（BotShell の top-header と同じ構造。スマホは ☰ でサイドバーを開く） -->
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
					<h2>
						{#if current}
							<button
								type="button"
								class="breadcrumb-link"
								title="{title}に戻る"
								onclick={() => navigateTo(base)}>{title}</button
							>
							<span class="breadcrumb-sep">›</span>
							{current.label}
						{:else}
							{title}
						{/if}
					</h2>
				</div>
			</header>

			{#if current}
				<!-- .management-overlay-body > section の表示規則（styles.css）を流用 -->
				<div class="management-overlay-body">
					{@render children(current.key)}
				</div>
			{:else}
				{@render hubHeader?.()}
				<!-- セクション一覧（設定ハブと同じ縦並びリスト。アイコン無し） -->
				<nav class="mgmt-list" aria-label={title}>
					{#each sections as s (s.key)}
						<button
							type="button"
							class="mgmt-item"
							onclick={() => navigateTo(`${base}/${s.key}`)}
						>
							<span class="mgmt-text">
								<span class="mgmt-label">{s.label}</span>
								<span class="mgmt-desc">{s.desc}</span>
							</span>
							<Icon name="chevron_right" class="mgmt-chevron" />
						</button>
					{/each}
				</nav>
			{/if}
		</div>
	</main>
</div>

<style>
	/* BotSelection / BotShell と同じ骨格（scoped のため各ページで宣言が必要） */
	.app-container {
		display: flex;
		min-height: 100vh;
	}
	/* 旧カード内の余白・内部スクロールを外し、ホームと同じくページ全体でスクロールさせる */
	.management-overlay-body {
		padding: 0;
		overflow: visible;
	}
	/* パンくずの親ページリンク（BotShell の .breadcrumb-link と同じ見た目） */
	.breadcrumb-link {
		background: none;
		border: none;
		padding: 0;
		font: inherit;
		color: var(--text-secondary);
		cursor: pointer;
		transition: color 0.15s ease;
	}
	.breadcrumb-link:hover {
		color: var(--text-primary);
		text-decoration: underline;
		text-underline-offset: 4px;
	}
	.breadcrumb-sep {
		color: var(--text-secondary);
		margin: 0 2px;
	}
	/* セクション一覧（BotSettingsHub と同じ縦並び・罫線区切り） */
	.mgmt-list {
		display: flex;
		flex-direction: column;
		border-top: 1px solid var(--border-divider);
	}
	.mgmt-item {
		display: flex;
		align-items: center;
		gap: 14px;
		width: 100%;
		padding: 16px 12px;
		text-align: left;
		cursor: pointer;
		font: inherit;
		color: var(--text-primary);
		background: none;
		border: none;
		border-bottom: 1px solid var(--border-divider);
		transition: background-color 0.15s ease;
	}
	.mgmt-item:hover,
	.mgmt-item:focus-visible {
		background-color: var(--surface-1dp);
	}
	.mgmt-item :global(.mgmt-chevron) {
		flex-shrink: 0;
		margin-left: auto;
		color: var(--text-secondary);
	}
	.mgmt-text {
		display: flex;
		flex-direction: column;
		gap: 4px;
		min-width: 0;
	}
	.mgmt-label {
		font-weight: 600;
	}
	.mgmt-desc {
		font-size: 0.82rem;
		color: var(--text-secondary);
	}
</style>
