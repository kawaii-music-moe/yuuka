<script lang="ts">
import type { Snippet } from "svelte";

// 汎用バッジ。既存 .status-badge をベースに任意の修飾クラスを付与できる。
// StatusChip はこの上に status→label/tone マップを載せた薄いラッパ。
interface Props {
	/** 追加する修飾クラス（例: "status-pending" / "status-sent"） */
	tone?: string;
	/** admin 系の丸ピル（.admin-status-badge）を使うか */
	pill?: boolean;
	class?: string;
	children?: Snippet;
}

let { tone = "", pill = false, class: klass = "", children }: Props = $props();

const base = $derived(pill ? "admin-status-badge" : "status-badge");
const classList = $derived([base, tone, klass].filter(Boolean).join(" "));
</script>

<span class={classList}>{@render children?.()}</span>
