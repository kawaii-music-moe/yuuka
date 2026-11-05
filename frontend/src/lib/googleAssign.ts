// Bot が使う Google アカウント（primary / 連携なし / 特定のアカウント）の選択肢と、
// その値と API（POST /api/integrated/grants/google）の body との変換。
// 「連携サービス」（IntegratedOverlay）と「Bot設定 › 連携の利用許可」（BotAccess）で共有する。
import type {
	IntegratedBotView,
	IntegratedGoogleAccountView,
} from "./api/types";

/** 特定のアカウントを選ぶ値の接頭辞（`acct:<accountId>`）。 */
const ACCOUNT_PREFIX = "acct:";

export interface GoogleGrantBody {
	botId: string;
	mode: "primary" | "none" | "account";
	accountId?: number;
}

/** Select の選択肢。値は googleAssignValue と同じ形。 */
export function googleAssignOptions(
	accounts: IntegratedGoogleAccountView[],
): { value: string; label: string }[] {
	return [
		{ value: "primary", label: "（primaryを使用）" },
		{ value: "none", label: "連携なし" },
		...accounts.map((a) => ({
			value: `${ACCOUNT_PREFIX}${a.id}`,
			label: a.email || `アカウント#${a.id}`,
		})),
	];
}

/** Bot の現在の設定（google_setting）を Select の値にする。 */
export function googleAssignValue(
	bot: Pick<IntegratedBotView, "google_setting">,
): string {
	const gs = bot.google_setting;
	return gs === "primary" || gs === "none" ? gs : `${ACCOUNT_PREFIX}${gs}`;
}

/** Select で選ばれた値を grantGoogle の body にする。 */
export function googleGrantBody(botId: string, value: string): GoogleGrantBody {
	if (value === "primary" || value === "none") return { botId, mode: value };
	return {
		botId,
		mode: "account",
		accountId: Number(value.slice(ACCOUNT_PREFIX.length)),
	};
}
