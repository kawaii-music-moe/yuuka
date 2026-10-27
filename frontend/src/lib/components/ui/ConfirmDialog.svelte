<script lang="ts" module>
import { writable } from "svelte/store";

// confirm() 置換用のストア駆動確認ダイアログ。
// 使い方:
//   1) App のルート付近に <ConfirmDialog /> を一度だけ設置。
//   2) 任意箇所で `const ok = await confirmDialog({ message: "削除しますか？" })`。
//   → Promise<boolean> を返す（OK=true / キャンセル=false）。
export interface ConfirmOptions {
	message: string;
	title?: string;
	/** OK ボタン文言 */
	confirmLabel?: string;
	/** キャンセルボタン文言 */
	cancelLabel?: string;
	/** OK を破壊的操作として赤ボタンにする */
	danger?: boolean;
}

interface ConfirmState extends ConfirmOptions {
	open: boolean;
	resolve: ((v: boolean) => void) | null;
}

const confirmStore = writable<ConfirmState>({
	open: false,
	message: "",
	resolve: null,
});

/** confirm() 相当。await で boolean を得る。 */
export function confirmDialog(opts: ConfirmOptions): Promise<boolean> {
	return new Promise<boolean>((resolve) => {
		confirmStore.set({ ...opts, open: true, resolve });
	});
}

function settle(result: boolean) {
	confirmStore.update((s) => {
		s.resolve?.(result);
		return { ...s, open: false, resolve: null };
	});
}

export const _confirmState = confirmStore;
export const _settle = settle;
</script>

<script lang="ts">
	import Modal from "./Modal.svelte";
	import Button from "./Button.svelte";

	let s = $state<ConfirmState>({ open: false, message: "", resolve: null });
	_confirmState.subscribe((v) => (s = v));
</script>

<Modal
	class="confirm-modal"
	open={s.open}
	title={s.title ?? "確認"}
	onclose={() => _settle(false)}
	closeOnBackdrop={true}
	closeOnEsc={true}
>
	<p class="confirm-message">{s.message}</p>

	{#snippet footer()}
		<Button variant="secondary" small class="confirm-cancel" onclick={() => _settle(false)}>
			{s.cancelLabel ?? "キャンセル"}
		</Button>
		<Button
			variant={s.danger ? "danger" : "primary"}
			small
			class="confirm-ok"
			onclick={() => _settle(true)}
		>
			{s.confirmLabel ?? "OK"}
		</Button>
	{/snippet}
</Modal>

<style>
	.confirm-message {
		font-size: 1rem;
		line-height: 1.6;
		margin-bottom: 24px;
		white-space: pre-wrap;
	}
	/* 区切り線と×を消し、テキストのキャンセル + 塗りの OK にする。 */
	:global(.modal.confirm-modal) {
		background-color: rgba(0, 0, 0, 0.85);
	}
	:global(.confirm-modal .modal-content) {
		padding: 28px 32px 24px;
		border: none;
		border-radius: 4px;
		background-color: var(--surface-0dp);
	}
	/* スマホのモーダル全画面化（styles.css）を確認モーダルでは打ち消し、中央の小さなカードにする。 */
	@media (max-width: 768px) {
		:global(.modal.confirm-modal) {
			padding: 16px;
		}
		:global(.confirm-modal .modal-content) {
			width: 100%;
			max-width: 460px;
			height: auto;
			max-height: 90vh;
			padding: 24px 20px 20px;
		}
	}
	:global(.confirm-modal .modal-header) {
		border-bottom: none;
		padding-bottom: 0;
	}
	:global(.confirm-modal .modal-header h3) {
		font-size: 1.4rem;
	}
	:global(.confirm-modal .btn-close) {
		display: none;
	}
	:global(.confirm-modal .modal-footer) {
		border-top: none;
		padding-top: 0;
		align-items: center;
		gap: 16px;
	}
	:global(.confirm-modal .confirm-cancel) {
		border: none;
		background: none;
	}
	:global(.confirm-modal .confirm-ok) {
		border: none;
	}
	:global(.confirm-modal .confirm-ok.btn-danger) {
		background-color: #f28b82;
		color: #601410;
	}
	:global(.confirm-modal .confirm-ok.btn-danger:hover) {
		background-color: #f6aea9;
	}
</style>
