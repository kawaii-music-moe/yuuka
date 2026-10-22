<script lang="ts">
// 候補つき入力（コンボボックス）。入力した ID・名前の部分一致で候補を絞り、選ぶと値は ID になる。
// 候補が無いときはただの ID 入力欄として使える。

import type { GuildOptionItem } from "../config/configTypes";

interface Props {
	options: GuildOptionItem[];
	value: string;
	placeholder?: string;
	/** 候補の名前の前に付ける記号（チャンネルなら "#"）。入力の先頭に付いていても無視する */
	prefix?: string;
}
let {
	options,
	value = $bindable(),
	placeholder = "",
	prefix = "",
}: Props = $props();

const listId = `combobox-${Math.random().toString(36).slice(2, 9)}`;
let open = $state(false);
let highlighted = $state(-1);

const filtered = $derived.by(() => {
	let q = value.trim().toLowerCase();
	if (prefix && q.startsWith(prefix)) q = q.slice(prefix.length);
	// 未入力、または候補を選び終えた直後（ID と完全一致）は全件を出して選び直せるようにする
	if (!q || options.some((opt) => opt.id === q)) return options;
	return options.filter(
		(opt) => opt.id.includes(q) || opt.name.toLowerCase().includes(q),
	);
});
const showList = $derived(open && filtered.length > 0);

function pick(opt: GuildOptionItem) {
	value = opt.id;
	open = false;
	highlighted = -1;
}

function onkeydown(e: KeyboardEvent) {
	if (e.key === "ArrowDown") {
		e.preventDefault();
		open = true;
		highlighted = Math.min(highlighted + 1, filtered.length - 1);
	} else if (e.key === "ArrowUp") {
		e.preventDefault();
		highlighted = Math.max(highlighted - 1, 0);
	} else if (e.key === "Enter" && showList && highlighted >= 0) {
		e.preventDefault();
		pick(filtered[highlighted]);
	} else if (e.key === "Escape") {
		open = false;
	}
}
</script>

<div class="combobox">
	<input
		type="text"
		class="mono"
		role="combobox"
		aria-expanded={showList}
		aria-controls={listId}
		aria-autocomplete="list"
		{placeholder}
		autocomplete="off"
		bind:value
		onfocus={() => (open = true)}
		onclick={() => (open = true)}
		oninput={() => {
			open = true;
			highlighted = -1;
		}}
		onblur={() => (open = false)}
		{onkeydown}
	/>
	{#if showList}
		<ul class="option-list" id={listId} role="listbox">
			{#each filtered as opt, i (opt.id)}
				<li
					role="option"
					aria-selected={i === highlighted}
					class:highlighted={i === highlighted}
					onmousedown={(e) => {
						// blur より先に選択を確定する
						e.preventDefault();
						pick(opt);
					}}
				>
					<span class="option-name">{prefix}{opt.name}</span>
					<span class="option-id mono">{opt.id}</span>
				</li>
			{/each}
		</ul>
	{/if}
</div>

<style>
	.combobox {
		position: relative;
		flex-grow: 1;
		min-width: 0;
	}
	.combobox input {
		width: 100%;
		box-sizing: border-box;
	}
	.mono {
		font-family: var(--font-family-mono);
	}
	.option-list {
		position: absolute;
		top: calc(100% + 4px);
		left: 0;
		right: 0;
		z-index: 20;
		max-height: 240px;
		overflow-y: auto;
		margin: 0;
		padding: 4px;
		list-style: none;
		background: var(--surface-1dp);
		border: 1px solid var(--border-divider);
		border-radius: 6px;
		box-shadow: 0 6px 18px rgba(0, 0, 0, 0.18);
	}
	.option-list li {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 12px;
		padding: 8px 10px;
		border-radius: 4px;
		cursor: pointer;
	}
	.option-list li:hover,
	.option-list li.highlighted {
		background: var(--surface-2dp, rgba(127, 127, 127, 0.12));
	}
	.option-name {
		font-size: 0.9rem;
		color: var(--text-primary);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.option-id {
		flex-shrink: 0;
		font-size: 0.8rem;
		color: var(--color-zinc-muted);
	}
</style>
