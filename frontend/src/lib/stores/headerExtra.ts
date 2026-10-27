// BotShell のヘッダー（タイトル下）に、表示中のタブが差し込む内容。
// タブ側は破棄時に null へ戻す。
import type { Snippet } from "svelte";
import { writable } from "svelte/store";

export const headerExtra = writable<Snippet | null>(null);
