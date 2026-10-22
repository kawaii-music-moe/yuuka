// BotShell のヘッダー（タイトル下・スコープバッジの位置）に、表示中のタブが差し込む内容。
// 設定されている間はスコープバッジの代わりに描画される。タブ側は破棄時に null へ戻す。
import type { Snippet } from "svelte";
import { writable } from "svelte/store";

export const headerExtra = writable<Snippet | null>(null);
