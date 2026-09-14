<script lang="ts">
	// ─────────────────────────────────────────────────────────────────────────
// ManagementOverlayShell — 全体管理ページ共通シェル。
// アカウント管理 / Bot統合管理 / 管理者設定はルーターで切り替える通常ページであり、
// モーダル用の .overlay / .management-overlay-card には載せない。
	// ─────────────────────────────────────────────────────────────────────────
	import type { Snippet } from "svelte";
	import { AdminPageHeader, Icon } from "$lib/components/ui";

	interface Props {
		/** ルート要素 id（モバイル用 CSS が参照） */
		id: string;
		/** ヘッダの Material Symbols アイコン名 */
		icon: string;
		title: string;
		children: Snippet;
	}
	let { id, icon, title, children }: Props = $props();
</script>


<div class="management-page" {id}>
	<AdminPageHeader class="management-page-header">
		<h1><Icon name={icon} size="1.6rem" /> {title}</h1>
	</AdminPageHeader>
	<main class="management-page-body">
		{@render children()}
	</main>
</div>

<style>
	.management-page {
		width: 100%;
		min-height: 100vh;
		min-height: 100dvh;
		background: var(--bg-secondary);
	}
	.management-page-body {
		width: min(1100px, 100%);
		margin: 0 auto;
		padding: 24px;
	}
	@media (max-width: 768px) {
		.management-page-body { padding: 14px 12px; }
	}
</style>
