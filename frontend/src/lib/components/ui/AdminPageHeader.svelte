<script lang="ts">
	import type { Snippet } from "svelte";
	import { openAdminNavigation } from "$lib/stores/adminNavigation";
	import Icon from "./Icon.svelte";

	interface Props {
		children: Snippet;
		class?: string;
	}

	let { children, class: className = "" }: Props = $props();
</script>

<header class={`admin-page-header ${className}`}>
	<button
		type="button"
		class="admin-menu-trigger"
		aria-label="メニューを開く"
		onclick={openAdminNavigation}
	>
		<Icon name="menu" />
	</button>
	<div class="admin-page-heading">{@render children()}</div>
</header>

<style>
	.admin-page-header {
		min-height: 58px;
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 0 16px;
		border-bottom: 1px solid var(--border-divider);
		background: var(--surface-1dp);
	}

	.admin-menu-trigger {
		width: 36px;
		height: 36px;
		display: grid;
		place-items: center;
		padding: 0;
		border: 0;
		border-radius: 4px;
		background: transparent;
		color: var(--color-primary);
		cursor: pointer;
	}

	.admin-menu-trigger:hover { background: rgba(21, 94, 239, 0.14); }
	.admin-page-heading { min-width: 0; flex: 1; }
	.admin-page-heading :global(h1),
	.admin-page-heading :global(h2) {
		margin: 0;
		font-size: 1rem;
		font-weight: 700;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
</style>
