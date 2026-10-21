// ─────────────────────────────────────────────────────────────────────────────
// Bot設定の各ページ（基本情報と共有 / 機能と AI / Discord）が共通で使う、
// 選択中 Bot の属性の取得（旧 BotConfig の loadBot）。activeBot には user_id・
// preset_display_name 等が無いため /api/bots の結果と突き合わせる。
// ─────────────────────────────────────────────────────────────────────────────
import { botAttributeApi } from "$lib/api/services";
import {
	type BotAttrView,
	type BotListResp,
	isDefaultBot,
} from "./configTypes";

/** 選択中 Bot の属性。既定 Bot（属性を持たない）・取得失敗・一覧に無いときは null。 */
export async function fetchBotAttr(botId: string): Promise<BotAttrView | null> {
	if (!botId || isDefaultBot(botId)) return null;
	try {
		const res = (await botAttributeApi.botList()) as BotListResp;
		return (res.bots ?? []).find((b) => b.id === botId) ?? null;
	} catch {
		return null;
	}
}
