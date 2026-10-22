<script lang="ts">
// ページ内タブ。1 ページの項目が多いとき、関連するまとまりごとに切り替えて一度に出す量を減らす
// （docs/design/settings-restructure.md「方針 3」）。中身の出し分けは呼び出し側が active で行う。
interface Tab {
	id: string;
	label: string;
}

interface Props {
	tabs: Tab[];
	/** 選択中のタブ id（bind:active で受け渡す） */
	active: string;
	/** tablist の読み上げ名 */
	label: string;
}

let { tabs, active = $bindable(), label }: Props = $props();

let buttons = $state<HTMLButtonElement[]>([]);

// 左右キーで隣のタブへ移る（WAI-ARIA Tabs パターン）。
function onkeydown(e: KeyboardEvent, index: number): void {
	const step = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
	if (!step) return;
	e.preventDefault();
	const next = (index + step + tabs.length) % tabs.length;
	active = tabs[next].id;
	buttons[next]?.focus();
}
</script>

<div class="page-tabs" role="tablist" aria-label={label}>
	{#each tabs as tab, i (tab.id)}
		<button
			type="button"
			role="tab"
			class="page-tab"
			class:active={active === tab.id}
			aria-selected={active === tab.id}
			tabindex={active === tab.id ? 0 : -1}
			bind:this={buttons[i]}
			onclick={() => (active = tab.id)}
			onkeydown={(e) => onkeydown(e, i)}
		>
			{tab.label}
		</button>
	{/each}
</div>

<style>
	.page-tabs {
		display: flex;
		gap: 4px;
		margin: 4px 0 16px;
		overflow-x: auto;
		border-bottom: 1px solid var(--border-divider);
	}
	.page-tab {
		flex-shrink: 0;
		padding: 10px 14px;
		font: inherit;
		font-weight: 600;
		color: var(--text-secondary);
		white-space: nowrap;
		cursor: pointer;
		background: none;
		border: none;
		border-bottom: 2px solid transparent;
		margin-bottom: -1px;
		transition:
			color 0.15s ease,
			border-color 0.15s ease;
	}
	.page-tab:hover {
		color: var(--text-primary);
	}
	.page-tab.active {
		color: var(--text-primary);
		border-bottom-color: var(--text-primary);
	}
</style>
