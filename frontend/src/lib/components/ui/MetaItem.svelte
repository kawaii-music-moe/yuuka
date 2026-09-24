<script lang="ts">
import type { Snippet } from "svelte";
import Icon from "./Icon.svelte";

// カードのメタ情報行。既存 .meta-item（mono・小サイズ・アイコン+テキスト）を再利用。
// 旧 buildMetaItem の置換。
interface Props {
	/** 左のアイコン名（Material Symbols） */
	icon?: string;
	/** テキスト（children 未指定時。自動エスケープ） */
	text?: string | number;
	title?: string;
	class?: string;
	children?: Snippet;
}

let { icon, text, title, class: klass = "", children }: Props = $props();
</script>

<span class="meta-item {klass}" {title}>
	{#if icon}<Icon name={icon} size={14} />{/if}
	{#if children}{@render children()}{:else}{text}{/if}
</span>
