<script lang="ts">
import type { Snippet } from "svelte";

// 共通カード枠。既存 .glass + .hover-lift を再利用。
interface Props {
	/** ホバー時にリフト効果（.hover-lift） */
	hover?: boolean;
	/** .glass を付けるか（false なら枠なし素の要素） */
	glass?: boolean;
	class?: string;
	children?: Snippet;
}

let {
	hover = true,
	glass = true,
	class: klass = "",
	children,
}: Props = $props();

const classList = $derived(
	[glass ? "glass" : "", hover ? "hover-lift" : "", klass]
		.filter(Boolean)
		.join(" "),
);
</script>

<div class={classList}>
	{@render children?.()}
</div>
