<script lang="ts">
// 一覧行の削除・却下ボタン（既定はゴミ箱アイコン・背景なし・ホバーで赤）。
import { Icon } from "$lib/components/ui";

interface Props {
	onclick: (e: MouseEvent) => void;
	title?: string;
	/** 読み上げ名（既定は「削除」） */
	label?: string;
	/** Material Symbols のアイコン名（既定はゴミ箱） */
	icon?: string;
}
let {
	onclick,
	title = "削除",
	label = "削除",
	icon = "delete",
}: Props = $props();
</script>

<button type="button" class="delete-button" {title} aria-label={label} {onclick}>
	<Icon name={icon} />
</button>

<style>
	.delete-button {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 32px;
		height: 32px;
		flex-shrink: 0;
		padding: 0;
		border: 0;
		border-radius: 6px;
		background: transparent;
		color: var(--color-zinc-muted);
		cursor: pointer;
		transition: color 0.18s ease;
	}
	.delete-button:hover {
		color: var(--color-error);
	}
	.delete-button :global(.material-symbols-outlined) {
		font-size: 18px;
	}
</style>
