// ─────────────────────────────────────────────────────────────────────────────
// Bot の起動・停止（旧 統合管理 › Bot ヘルスの操作）。設定ではなく運用操作なので、
// 再構成（docs/design/settings-restructure.md）で Bot一覧のカードとダッシュボードへ移した。
// ─────────────────────────────────────────────────────────────────────────────
import { integratedApi } from "$lib/api/services";
import type { ChipStatus } from "$lib/components/ui";

export type RuntimeAction = "start" | "stop" | "restart";

/** 起動・停止の判定に使う Bot の稼働情報（/api/bots・/api/integrated/overview の共通部分）。 */
export interface RuntimeBot {
	id: string;
	user_id?: string;
	has_token: boolean;
	running: boolean;
	connected: boolean;
	suspended: boolean | number;
}

/**
 * 起動・停止を操作できるか。API はオーナー本人の Bot だけを受け付け（共有相手・system_default は不可）、
 * トークン未設定・管理者による差し押さえ中は動かせない。
 */
export function canControlRuntime(
	bot: RuntimeBot,
	userId: string | undefined,
): boolean {
	return (
		!!userId &&
		bot.user_id === userId &&
		bot.id !== "system_default" &&
		bot.has_token &&
		!bot.suspended
	);
}

/** 状態チップの表示（旧 IntegratedOverlay の botChip）。 */
export function runtimeChip(bot: RuntimeBot): {
	status: ChipStatus;
	label: string;
} {
	if (bot.suspended) return { status: "stopped", label: "停止中(管理者)" };
	if (bot.connected) return { status: "running", label: "稼働中" };
	if (bot.running) return { status: "pending", label: "接続中…" };
	if (!bot.has_token) return { status: "unset", label: "トークン未設定" };
	return { status: "unset", label: "停止" };
}

/**
 * 操作を送り、状態を引き直すまでに待つ時間（ms）を返す。stop は即時反映、start/restart は
 * Discord の接続確立を待つため少し遅らせる。
 */
export async function controlRuntime(
	action: RuntimeAction,
	botId: string,
): Promise<number> {
	if (action === "start") await integratedApi.startBot(botId);
	else if (action === "stop") await integratedApi.stopBot(botId);
	else await integratedApi.restartBot(botId);
	return action === "stop" ? 300 : 1500;
}
