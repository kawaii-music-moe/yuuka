<script lang="ts">
import type { Snippet } from "svelte";
import Icon from "./Icon.svelte";

// タグ表示チップ。既存 .tag-chip を再利用。任意で削除ボタン付き。
interface Props {
	/** タグ文字列（children 未指定時に表示。自動エスケープ） */
	label?: string;
	/** 削除ボタンを表示し、押下で onremove を呼ぶ */
	removable?: boolean;
	onremove?: () => void;
	class?: string;
	children?: Snippet;
}

let {
	label,
	removable = false,
	onremove,
	class: klass = "",
	children,
}: Props = $props();
</script>

<span class="tag-chip {klass}">
	{#if children}{@render children()}{:else}{label}{/if}
	{#if removable}
		<button
			type="button"
			class="tag-chip-remove"
			aria-label="削除"
			onclick={onremove}
		>
			<Icon name="close" size={12} />
		</button>
	{/if}
</span>

<style>
	.tag-chip-remove {
		background: none;
		border: none;
		padding: 0;
		margin-left: 4px;
		cursor: pointer;
		color: inherit;
		display: inline-flex;
		align-items: center;
		vertical-align: middle;
		opacity: 0.7;
	}
	.tag-chip-remove:hover {
		opacity: 1;
	}
</style>
