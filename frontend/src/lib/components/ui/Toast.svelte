<script lang="ts">
import {
	removeToast,
	type Toast as ToastData,
	type ToastKind,
	toasts,
} from "$lib/stores/toast";
import Icon from "./Icon.svelte";

// stores/toast を購読して画面右下にトースト群を表示。
// 統合担当が App のルートに一度だけ設置する。alert() 置換用。
// §11.4: message は文字列補間のみ（{@html} 禁止）。

const ICONS: Record<ToastKind, string> = {
	info: "info",
	success: "check_circle",
	error: "error",
	warning: "warning",
};

let list = $state<ToastData[]>([]);
toasts.subscribe((v) => (list = v));
</script>

<div class="toast-container" role="region" aria-live="polite" aria-label="通知">
	{#each list as t (t.id)}
		<div class="toast toast-{t.kind}" role="status">
			<Icon name={ICONS[t.kind]} size={18} class="toast-icon" />
			<span class="toast-message">{t.message}</span>
			<button
				type="button"
				class="toast-close"
				aria-label="閉じる"
				onclick={() => removeToast(t.id)}
			>
				<Icon name="close" size={16} />
			</button>
		</div>
	{/each}
</div>

<style>
	.toast-container {
		position: fixed;
		bottom: 20px;
		right: 20px;
		z-index: 10001; /* .modal(10000) より前面 */
		display: flex;
		flex-direction: column;
		gap: 10px;
		max-width: min(360px, calc(100vw - 40px));
		pointer-events: none;
	}
	.toast {
		pointer-events: auto;
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 12px 14px;
		border-radius: var(--radius);
		background-color: var(--surface-4dp, #2a2a2e);
		border: 1px solid var(--border-matte);
		box-shadow: 0 8px 20px rgba(0, 0, 0, 0.4);
		font-size: 0.85rem;
		color: var(--text-primary, #e5e5e5);
		animation: item-fade-in 0.25s ease forwards;
	}
	.toast-message {
		flex: 1;
		min-width: 0;
		overflow-wrap: break-word;
	}
	.toast :global(.toast-icon) {
		flex-shrink: 0;
	}
	.toast-close {
		background: none;
		border: none;
		padding: 0;
		cursor: pointer;
		color: var(--color-zinc-muted);
		display: inline-flex;
		flex-shrink: 0;
	}
	.toast-close:hover {
		color: var(--text-primary, #fff);
	}
	.toast-success :global(.toast-icon) {
		color: var(--color-green, #22c55e);
	}
	.toast-error :global(.toast-icon) {
		color: var(--color-red, #ef4444);
	}
	.toast-warning :global(.toast-icon) {
		color: #fbbf24;
	}
	.toast-info :global(.toast-icon) {
		color: var(--color-primary);
	}
</style>
