<script lang="ts">
// 追加ボタン（既定は + アイコン。追加するものに合わせて差し替える・背景なし）。保存の ✓ ボタンと同じ見た目。
import { Icon } from "$lib/components/ui";

interface Props {
	onclick: () => void;
	title?: string;
	disabled?: boolean;
	/** Material Symbols のアイコン名（既定は +） */
	icon?: string;
}
let {
	onclick,
	title = "追加",
	disabled = false,
	icon = "add",
}: Props = $props();
</script>

<button type="button" class="icon-action" {title} aria-label={title} {disabled} {onclick}>
	<Icon name={icon} />
</button>
