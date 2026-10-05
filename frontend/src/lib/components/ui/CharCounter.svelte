<script lang="ts">
// 入力文字数カウンタ。textarea/input の value.length を max と対比表示。
// max 超過時は over クラスで警告色。
interface Props {
	/** 現在値（文字列 or 長さ数値） */
	value: string | number;
	/** 上限文字数 */
	max: number;
	class?: string;
}

let { value, max, class: klass = "" }: Props = $props();

const count = $derived(typeof value === "number" ? value : value.length);
const over = $derived(count > max);
</script>

<span class="char-counter {klass}" class:over aria-live="polite">
	{count} / {max}
</span>

<style>
	.char-counter {
		font-family: var(--font-family-mono);
		font-size: 0.7rem;
		color: var(--color-zinc-muted);
	}
	.char-counter.over {
		color: var(--color-red);
	}
</style>
