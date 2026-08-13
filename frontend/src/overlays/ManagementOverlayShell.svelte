<script lang="ts">
	// ─────────────────────────────────────────────────────────────────────────
	// ManagementOverlayShell — 全体管理オーバーレイ共通シェル。
	// アカウント管理 / Bot統合管理 / 管理者設定の3画面で重複していた
	// .overlay > .management-overlay-card > header(タイトル + Bot選択に戻る) + body
	// の骨格を集約する。styles.css の #id 別モバイル全画面規則
	// （#integrated-overlay / #admin-overlay）が効くよう id は props で受ける。
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

<div class="overlay active" {id}>
	<div class="management-overlay-card">
		<AdminPageHeader>
			<h1><Icon name={icon} size="1.6rem" /> {title}</h1>
		</AdminPageHeader>
		<div class="management-overlay-body">
			{@render children()}
		</div>
	</div>
</div>
