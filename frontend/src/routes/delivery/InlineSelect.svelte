<script lang="ts" generics="T extends string">
// 枠つき入力（joined-field）の中に置くプルダウン。見た目は Discord 設定の候補リスト
// （OptionCombobox）に合わせた独自実装で、ブラウザ標準の白い候補表示を出さない。

interface Props {
	id: string;
	options: { value: T; label: string }[];
	value: T;
	class?: string;
	/** 読み上げ名（label 要素と関連付けない場合） */
	ariaLabel?: string;
	/** ユーザー操作で値が変わったときに呼ぶ */
	onchange?: (value: T) => void;
}
let {
	id,
	options,
	value = $bindable(),
	class: className = "",
	ariaLabel,
	onchange,
}: Props = $props();

function choose(v: T) {
	value = v;
	onchange?.(v);
}

let open = $state(false);
let root: HTMLDivElement;
// 下に候補を出す余白が足りなければ、上に出す
let dropUp = $state(false);

$effect(() => {
	if (!open || !root) return;
	const rect = root.getBoundingClientRect();
	const listHeight = Math.min(240, options.length * 36 + 8);
	const below = window.innerHeight - rect.bottom;
	dropUp = below < listHeight + 8 && rect.top > below;
});
const listId = $derived(`${id}-list`);

function onkeydown(e: KeyboardEvent) {
	if (e.key === "ArrowDown" || e.key === "ArrowUp") {
		e.preventDefault();
		const i = options.findIndex((o) => o.value === value);
		const next = i + (e.key === "ArrowDown" ? 1 : -1);
		choose(options[Math.max(0, Math.min(options.length - 1, next))].value);
		open = true;
	} else if ((e.key === "Escape" || e.key === "Enter") && open) {
		e.preventDefault();
		open = false;
	}
}
</script>

<div class="inline-select {className}" bind:this={root}>
	<button
		{id}
		type="button"
		class="trigger"
		aria-label={ariaLabel}
		aria-haspopup="listbox"
		aria-expanded={open}
		aria-controls={listId}
		onclick={() => (open = !open)}
		onblur={() => (open = false)}
		{onkeydown}
	>
		<span>{options.find((o) => o.value === value)?.label}</span>
		<span class="material-symbols-outlined chevron" aria-hidden="true">expand_more</span>
	</button>
	{#if open}
		<ul class="option-list" class:up={dropUp} id={listId} role="listbox">
			{#each options as opt (opt.value)}
				<li
					role="option"
					aria-selected={opt.value === value}
					class:highlighted={opt.value === value}
					onmousedown={(e) => {
						// blur より先に選択を確定する
						e.preventDefault();
						choose(opt.value);
						open = false;
					}}
				>
					{opt.label}
				</li>
			{/each}
		</ul>
	{/if}
</div>

<style>
	.inline-select {
		position: relative;
		display: flex;
	}
	.trigger {
		display: flex;
		flex: 1;
		align-items: center;
		justify-content: space-between;
		gap: 8px;
		padding: 12px;
		border: 0;
		background: transparent;
		color: var(--text-high);
		font: inherit;
		font-size: 0.95rem;
		white-space: nowrap;
		cursor: pointer;
	}
	.chevron {
		font-size: 20px;
		color: var(--text-secondary);
	}
	.option-list {
		position: absolute;
		top: calc(100% + 4px);
		left: 0;
		min-width: 100%;
		z-index: 20;
		margin: 0;
		padding: 4px;
		list-style: none;
		background: var(--surface-1dp);
		border: 1px solid var(--border-divider);
		border-radius: 6px;
		box-shadow: 0 6px 18px rgba(0, 0, 0, 0.18);
		max-height: 240px;
		overflow-y: auto;
	}
	.option-list.up {
		top: auto;
		bottom: calc(100% + 4px);
	}
	.option-list li {
		padding: 8px 10px;
		border-radius: 4px;
		font-size: 0.9rem;
		color: var(--text-primary);
		white-space: nowrap;
		cursor: pointer;
	}
	.option-list li:hover,
	.option-list li.highlighted {
		background: var(--surface-2dp, rgba(127, 127, 127, 0.12));
	}
</style>
