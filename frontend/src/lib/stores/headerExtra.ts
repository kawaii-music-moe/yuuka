// BotShell のヘッダー（タイトル下）に、表示中のタブが差し込む内容。
// タブ側は破棄時に null へ戻す。
import { onDestroy, type Snippet } from "svelte";
import { writable } from "svelte/store";

export const headerExtra = writable<Snippet | null>(null);

/**
 * 表示中のタブからヘッダーへ content を差し込み、タブの破棄時に外す。
 * onDestroy を使うため、コンポーネントの初期化中（<script> の直下）で呼ぶこと。
 */
export function setHeaderExtra(content: Snippet): void {
	headerExtra.set(content);
	onDestroy(() => headerExtra.set(null));
}
