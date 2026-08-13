import type { Bot } from "$lib/stores/activeBot";

function clientOrigin(): string {
	if (typeof window === "undefined") return "";
	const url = new URL(window.location.href);
	if (url.port === "5174") url.port = "5173";
	return url.origin;
}

/** 選択中 Bot を Client へ引き渡して開く。本番では同一オリジンのルートへ遷移する。 */
export function openClientForBot(bot: Exclude<Bot, null>): void {
	if (typeof window === "undefined") return;
	const target = new URL("/", clientOrigin());
	target.searchParams.set("botId", bot.id);
	window.location.assign(target);
}
