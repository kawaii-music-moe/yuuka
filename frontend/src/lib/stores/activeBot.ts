import { writable } from "svelte/store";

// §9.2 選択中 Bot ストア（localStorage 同期 + 旧4キー one-time マイグレーション）
export type Bot = {
	id: string;
	name: string;
	avatar: string;
	preset: string;
} | null;

const KEY = "currentBot";
/** システムデフォルト Bot。案内役で個人データを持たないため選択できない（issue #140）。 */
const SYSTEM_DEFAULT_BOT_ID = "system_default";

function load(): Bot {
	if (typeof window === "undefined") return null;
	const raw = localStorage.getItem(KEY);
	if (raw) {
		try {
			const bot = JSON.parse(raw) as Bot;
			// 以前システム Bot を選んでいた場合は未選択に戻す（Bot 一覧へ誘導される）。
			return bot?.id === SYSTEM_DEFAULT_BOT_ID ? null : bot;
		} catch {
			/* fallthrough: 破損時は旧4キー移行を試す */
		}
	}
	// one-time マイグレーション: 旧4キー → 新オブジェクト
	const id = localStorage.getItem("currentBotId");
	if (id && id !== SYSTEM_DEFAULT_BOT_ID) {
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
	activeBot.set(bot?.id === SYSTEM_DEFAULT_BOT_ID ? null : bot);
}
