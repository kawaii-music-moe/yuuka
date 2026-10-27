// 選択中 Bot で無効化されている機能のタブ（モジュールの settingsKey = BotTab 名）。
// BotShell のサイドバーと設定ハブが参照し、「有効な機能」で無効にした機能の入口を隠す。
// BotModulesCard が読み込み・保存のたびに syncDisabledTabs で更新するので、切り替えは即時に反映される。
import { writable } from "svelte/store";
import { botAttributeApi } from "$lib/api/services";

type ModuleLike = { settingsKey?: string | null; enabled: boolean };

export const disabledTabs = writable<ReadonlySet<string>>(new Set());

/** モジュール一覧から無効なタブ集合を作って反映する。 */
export function syncDisabledTabs(modules: readonly ModuleLike[]): void {
	disabledTabs.set(
		new Set(
			modules
				.filter((m) => !m.enabled && m.settingsKey)
				.map((m) => m.settingsKey as string),
		),
	);
}

let latestBotId = "";

/** Bot を切り替えたときに取り直す（取得できなければ何も隠さない）。 */
export async function refreshDisabledTabs(botId: string): Promise<void> {
	latestBotId = botId;
	disabledTabs.set(new Set());
	if (!botId) return;
	try {
		const res = (await botAttributeApi.modules(botId)) as {
			success: boolean;
			modules?: ModuleLike[];
		};
		if (botId === latestBotId && res.success)
			syncDisabledTabs(res.modules ?? []);
	} catch {
		/* 取得失敗時はすべて表示のまま */
	}
}
