import { writable } from "svelte/store";

// §9.2 選択中 Bot ストア（localStorage 同期 + 旧4キー one-time マイグレーション）
export type Bot = {
	id: string;
	name: string;
	avatar: string;
	preset: string;
} | null;

const KEY = "currentBot";

function clearStoredBot(): void {
	localStorage.removeItem(KEY);
	localStorage.removeItem("currentBotId");
	localStorage.removeItem("currentBotName");
	localStorage.removeItem("currentBotAvatar");
	localStorage.removeItem("currentBotPreset");
}

function isRetiredBotId(id: string | undefined): boolean {
	return !id || id === "system_default" || id.startsWith("bot_default_");
}

function load(): Bot {
	if (typeof window === "undefined") return null;
	const raw = localStorage.getItem(KEY);
	if (raw) {
		try {
			const bot = JSON.parse(raw) as Bot;
			if (bot && isRetiredBotId(bot.id)) {
				clearStoredBot();
				return null;
			}
			return bot;
		} catch {
			/* fallthrough: 破損時は旧4キー移行を試す */
		}
	}
	// one-time マイグレーション: 旧4キー → 新オブジェクト
	const id = localStorage.getItem("currentBotId");
	if (id && !isRetiredBotId(id)) {
		const migrated: Bot = {
			id,
			name: localStorage.getItem("currentBotName") ?? "",
			avatar: localStorage.getItem("currentBotAvatar") ?? "",
			preset: localStorage.getItem("currentBotPreset") ?? "",
		};
		localStorage.setItem(KEY, JSON.stringify(migrated)); // 書き戻し
		return migrated;
	}
	if (id) clearStoredBot();
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
		clearStoredBot();
	}
});

/** Bot 選択ヘルパ。selectBot() の localStorage 書き込みは activeBot.set() に集約。 */
export function selectBot(bot: Bot): void {
	activeBot.set(bot);
}
