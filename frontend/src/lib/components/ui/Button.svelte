<script lang="ts">
import type { Snippet } from "svelte";

// 既存 .btn 系クラスへマップする共通ボタン。
// variant → class:
//   primary   → .btn .btn-primary
//   secondary → .btn .btn-secondary
//   danger    → .btn .btn-danger
//   mini      → .btn-mini
//   trash     → .btn-trash
//   icon      → .btn-icon
//   icon-sm   → .btn-icon-sm
export type ButtonVariant =
	| "primary"
	| "secondary"
	| "danger"
	| "mini"
	| "trash"
	| "icon"
	| "icon-sm";

interface Props {
	variant?: ButtonVariant;
	type?: "button" | "submit" | "reset";
	disabled?: boolean;
	/** 小サイズ（.btn 系にのみ .btn-sm を追加） */
	small?: boolean;
	/** 幅いっぱい（.btn 系にのみ .btn-block を追加） */
	block?: boolean;
	title?: string;
	"aria-label"?: string;
	class?: string;
	onclick?: (e: MouseEvent) => void;
	children?: Snippet;
}

let {
	variant = "primary",
	type = "button",
	disabled = false,
	small = false,
	block = false,
	title,
	"aria-label": ariaLabel,
	class: klass = "",
	onclick,
	children,
}: Props = $props();

// 既存クラス体系に合わせて class 列を構築。
const classList = $derived.by(() => {
	const parts: string[] = [];
	switch (variant) {
		case "primary":
			parts.push("btn", "btn-primary");
			break;
		case "secondary":
			parts.push("btn", "btn-secondary");
			break;
		case "danger":
			parts.push("btn", "btn-danger");
			break;
		case "mini":
			parts.push("btn-mini");
			break;
		case "trash":
			parts.push("btn-trash");
			break;
		case "icon":
			parts.push("btn-icon");
			break;
		case "icon-sm":
			parts.push("btn-icon-sm");
			break;
	}
	const isFullBtn =
		variant === "primary" || variant === "secondary" || variant === "danger";
	if (isFullBtn && small) parts.push("btn-sm");
	if (isFullBtn && block) parts.push("btn-block");
	if (klass) parts.push(klass);
	return parts.join(" ");
});
</script>

<button
	{type}
	class={classList}
	{disabled}
	{title}
	aria-label={ariaLabel}
	{onclick}
>
	{@render children?.()}
</button>
