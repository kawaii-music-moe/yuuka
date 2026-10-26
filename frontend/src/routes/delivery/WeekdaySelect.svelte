<script lang="ts">
// 曜日の複数選択プルダウン。見た目は InlineSelect と同じで、選んでも閉じずにトグルできる。
// 値は cron の曜日番号（0=日〜6=土）。

interface Props {
	id: string;
	value: number[];
	onchange?: (value: number[]) => void;
}
let { id, value = $bindable(), onchange }: Props = $props();

const DAYS = ["日", "月", "火", "水", "木", "金", "土"];
// 表示は月曜始まり
const ORDER = [1, 2, 3, 4, 5, 6, 0];

let open = $state(false);
let root: HTMLDivElement;
// 下に候補を出す余白が足りなければ、上に出す
let dropUp = $state(false);

$effect(() => {
	if (!open || !root) return;
	const rect = root.getBoundingClientRect();
	const listHeight = Math.min(240, ORDER.length * 36 + 8);
	const below = window.innerHeight - rect.bottom;
	dropUp = below < listHeight + 8 && rect.top > below;
});
const listId = $derived(`${id}-list`);
const summary = $derived(
	value.length === 0
		? "曜日を選択"
		: ORDER.filter((d) => value.includes(d))
				.map((d) => DAYS[d])
				.join("・"),
);

function toggle(d: number) {
	value = value.includes(d)
		? value.filter((v) => v !== d)
		: [...value, d].sort((a, b) => a - b);
	onchange?.(value);
}
</script>

<div class="inline-select" bind:this={root}>
	<button
		{id}
		type="button"
		class="trigger"
		aria-label="曜日"
		aria-haspopup="listbox"
		aria-expanded={open}
		aria-controls={listId}
		onclick={() => (open = !open)}
		onblur={() => (open = false)}
		onkeydown={(e) => {
			if (e.key === "Escape" && open) {
				e.preventDefault();
				open = false;
			}
		}}
	>
		<span class:placeholder={value.length === 0}>{summary}</span>
		<span class="material-symbols-outlined chevron" aria-hidden="true">expand_more</span>
	</button>
	{#if open}
		<ul class="option-list" class:up={dropUp} id={listId} role="listbox" aria-multiselectable="true">
			{#each ORDER as d (d)}
				<li
					role="option"
					aria-selected={value.includes(d)}
					class:selected={value.includes(d)}
					onmousedown={(e) => {
						// blur で閉じないようにしてトグルだけ行う
						e.preventDefault();
						toggle(d);
					}}
				>
					<span class="material-symbols-outlined check" aria-hidden="true"
						>{value.includes(d) ? "check_box" : "check_box_outline_blank"}</span
					>
					{DAYS[d]}曜日
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
	.placeholder {
		color: var(--text-secondary);
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
	}
	.option-list.up {
		top: auto;
		bottom: calc(100% + 4px);
	}
	.option-list li {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 8px 10px;
		border-radius: 4px;
		font-size: 0.9rem;
		color: var(--text-primary);
		white-space: nowrap;
		cursor: pointer;
	}
	.option-list li:hover {
		background: var(--surface-2dp, rgba(127, 127, 127, 0.12));
	}
	.check {
		font-size: 18px;
		color: var(--text-secondary);
	}
	.selected .check {
		color: var(--color-primary);
	}
</style>
