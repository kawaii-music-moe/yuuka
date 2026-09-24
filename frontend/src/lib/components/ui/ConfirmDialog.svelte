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
	open={s.open}
	title={s.title ?? "確認"}
	onclose={() => _settle(false)}
	closeOnBackdrop={true}
	closeOnEsc={true}
>
	<p class="confirm-message">{s.message}</p>

	{#snippet footer()}
		<Button variant="secondary" small onclick={() => _settle(false)}>
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
		font-size: 0.9rem;
		line-height: 1.6;
		margin-bottom: 20px;
		white-space: pre-wrap;
	}
	/* .modal-footer は gap 未定義のためボタン間隔を補う。 */
	:global(.modal-footer .confirm-ok) {
		margin-left: 8px;
	}
</style>
