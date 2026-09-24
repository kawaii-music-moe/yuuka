<script lang="ts">
import type { Snippet } from "svelte";
import Icon from "./Icon.svelte";

// 「…がありません」の共通空状態表示。各 fetchXxx に散在していた文言を集約。
interface Props {
	/** Material Symbols アイコン名 */
	icon?: string;
	/** 表示メッセージ（文字列。自動エスケープ） */
	message: string;
	class?: string;
	/** 補足アクション等（任意） */
	children?: Snippet;
}

let { icon = "inbox", message, class: klass = "", children }: Props = $props();
</script>

<div class="empty-state {klass}">
	{#if icon}
		<Icon name={icon} class="empty-state-icon" size={40} />
	{/if}
	<p class="empty-state-message">{message}</p>
	{@render children?.()}
</div>

<style>
	/* 汎用最小スタイル。styles.css に .empty-state 定義が無いため部品側で最小補完。
	   テーマ変数のみ参照しトークン整合を保つ。 */
	.empty-state {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 8px;
		padding: 32px 16px;
		text-align: center;
		color: var(--color-zinc-muted);
	}
	.empty-state :global(.empty-state-icon) {
		opacity: 0.5;
	}
	.empty-state-message {
		font-size: 0.85rem;
	}
</style>
