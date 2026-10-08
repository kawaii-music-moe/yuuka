import { writable } from "svelte/store";

// §9.2 選択中 Bot ストア（localStorage 同期 + 旧4キー one-time マイグレーション）
export type Bot = {
	id: string;
	name: string;
	avatar: string;
	preset: string;
} | null;

/**
 * システム Bot。Yuuka の使い方・設定の案内役で個人データを持たないため、管理画面では選択できない
 * （Bot 一覧に出さず、以前に選んだまま保存されていても読み込み時に捨てる）。
 * トークン更新などの管理は「管理者設定」で行う。
 */
export const SYSTEM_BOT_ID = "system_default";

/** `/api/bots` の一覧（自分の Bot・共有された Bot・システム Bot）から、選択できる Bot だけを残す。 */
export function selectableBots<T extends { id: string }>(bots: T[]): T[] {
	return bots.filter((bot) => bot.id !== SYSTEM_BOT_ID);
}

const KEY = "currentBot";

function loadStored(): Bot {
	if (typeof window === "undefined") return null;
	const raw = localStorage.getItem(KEY);
	if (raw) {
		try {
			return JSON.parse(raw) as Bot;
		} catch {
			/* fallthrough: 破損時は旧4キー移行を試す */
		}
	}
	// one-time マイグレーション: 旧4キー → 新オブジェクト
	const id = localStorage.getItem("currentBotId");
	if (id) {
		const migrated: Bot = {
			id,
			name: localStorage.getItem("currentBotName") ?? "",
			avatar: localStorage.getItem("currentBotAvatar") ?? "",
			preset: localStorage.getItem("currentBotPreset") ?? "",
		};
		localStorage.setItem(KEY, JSON.stringify(migrated)); // 書き戻し
		return migrated;
	}
	return null;
}

// 以前にシステム Bot を選んでいた場合は未選択に戻す（/bot/* は App のガードで Bot 一覧へ）。
function load(): Bot {
	const bot = loadStored();
	return bot?.id === SYSTEM_BOT_ID ? null : bot;
}

export const activeBot = writable<Bot>(load());

activeBot.subscribe((b) => {
	if (typeof window === "undefined") return;
	if (b) {
		localStorage.setItem(KEY, JSON.stringify(b));
		// 旧タブ/旧sw.jsと共存する移行期間中は旧4キーも書き続けて後方互換を保つ
		localStorage.setItem("currentBotId", b.id);
		localStorage.setItem("currentBotName", b.name);
		localStorage.setItem("currentBotAvatar", b.avatar);
		localStorage.setItem("currentBotPreset", b.preset);
	} else {
		localStorage.removeItem(KEY);
	}
});

/** Bot 選択ヘルパ。selectBot() の localStorage 書き込みは activeBot.set() に集約。 */
export function selectBot(bot: Bot): void {
	activeBot.set(bot);
}
