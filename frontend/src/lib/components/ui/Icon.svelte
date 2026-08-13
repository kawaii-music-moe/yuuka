<script lang="ts">
	// Material Symbols アイコン共通部品。
	// 旧 <span class="material-symbols-outlined">name</span> の手組みを置換する。
	interface Props {
		/** Material Symbols のアイコン名（例: "delete", "check", "add"） */
		name: string;
		/** フォントサイズ。数値(px) or CSS 長さ文字列。未指定なら CSS 既定。 */
		size?: number | string;
		/** filled 表示（font-variation-settings 'FILL' 1） */
		fill?: boolean;
		/** 追加クラス */
		class?: string;
	}

	let { name, size, fill = false, class: klass = "" }: Props = $props();

	const sizeCss = $derived(
		size === undefined ? "" : typeof size === "number" ? `${size}px` : size,
	);
	const style = $derived(
		[
			sizeCss ? `font-size:${sizeCss}` : "",
			fill ? "font-variation-settings:'FILL' 1" : "",
		]
			.filter(Boolean)
			.join(";"),
	);
</script>

<span class="material-symbols-outlined {klass}" {style} aria-hidden="true"
	>{name}</span
>

<style>
	.material-symbols-outlined {
		user-select: none;
		-webkit-user-select: none;
		-moz-user-select: none;
	}
</style>
