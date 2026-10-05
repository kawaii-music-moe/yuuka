import { writable } from "svelte/store";

// 簡易トーストストア（Toast.svelte / ConfirmDialog が購読して使う）
export type ToastKind = "info" | "success" | "error" | "warning";

export type Toast = {
	id: number;
	kind: ToastKind;
	message: string;
	/** 自動消去までの ms。0 以下で永続（手動 removeToast まで残す）。 */
	timeout: number;
};

export const toasts = writable<Toast[]>([]);

let nextId = 1;

/**
 * トーストを追加。id を返すので呼び出し側で removeToast(id) できる。
 * timeout > 0 の場合は自動消去タイマーを張る。
 */
export function pushToast(
	message: string,
	kind: ToastKind = "info",
	timeout = 4000,
): number {
	const id = nextId++;
	const toast: Toast = { id, kind, message, timeout };
	toasts.update((list) => [...list, toast]);
	if (timeout > 0 && typeof window !== "undefined") {
		window.setTimeout(() => removeToast(id), timeout);
	}
	return id;
}

export function removeToast(id: number): void {
	toasts.update((list) => list.filter((t) => t.id !== id));
}

export function clearToasts(): void {
	toasts.set([]);
}
