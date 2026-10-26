<script lang="ts">
// ? アイコンのヘルプ。ホバーで説明を出し、クリックで開いたままにできる。
interface Props {
	id: string;
	text: string;
	label?: string;
	/** 吹き出しの寄せ方（右端に置くときは "end" で画面外へはみ出さないようにする） */
	align?: "center" | "end";
}
let { id, text, label = "説明", align = "center" }: Props = $props();

let open = $state(false);
</script>

<span class="help">
	<button
		type="button"
		class="help-button"
		aria-label={label}
		aria-describedby={id}
		aria-expanded={open}
		onclick={() => (open = !open)}
		onblur={() => (open = false)}
	>
		<span class="material-symbols-outlined" aria-hidden="true">help</span>
	</button>
	<span {id} role="tooltip" class="help-tip" class:open class:end={align === "end"}>{text}</span>
</span>

<style>
	.help {
		position: relative;
		display: flex;
	}
	.help-button {
		display: flex;
		align-items: center;
		padding: 0;
		border: 0;
		background: transparent;
		color: var(--text-secondary);
		cursor: pointer;
	}
	.help-button .material-symbols-outlined {
		font-size: 20px;
		line-height: 1;
	}
	.help-tip {
		position: absolute;
		left: 50%;
		bottom: calc(100% + 6px);
		transform: translateX(-50%);
		z-index: 20;
		width: max-content;
		max-width: min(320px, 80vw);
		padding: 6px 10px;
		font-size: 0.8rem;
		font-weight: 400;
		line-height: 1.6;
		color: var(--text-primary);
		background: var(--surface-1dp);
		border: 1px solid var(--border-divider);
		border-radius: 6px;
		box-shadow: 0 6px 18px rgba(0, 0, 0, 0.18);
		opacity: 0;
		visibility: hidden;
		pointer-events: none;
		transition: opacity 0.15s ease;
	}
	.help-tip.end {
		left: auto;
		right: 0;
		transform: none;
	}
	.help:hover .help-tip,
	.help-tip.open {
		opacity: 1;
		visibility: visible;
	}
</style>
