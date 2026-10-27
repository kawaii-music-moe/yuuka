<script lang="ts">
// 伏せ字の値（トークン・API キーなど）が登録済みのとき、ラベルの横に出すチェック。
// 値そのものは返さない API の「設定済みか」（has_token など）を渡す。
import Icon from "./Icon.svelte";

interface Props {
	set: boolean | null | undefined;
}
let { set }: Props = $props();
</script>

{#if set}<span class="secret-set-mark" role="img" aria-label="登録済み" title="登録済み"
		><Icon name="check_circle" size={16} /></span
	>{/if}
