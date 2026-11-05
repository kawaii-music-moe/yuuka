// チャットの送信・応答待ちの表示（チャット画面と音声画面で共通）。
import { ChatReplyTimeoutError } from "../api/chatPolling";
import { ApiError } from "../api/http";

/** この時間を超えても応答が届かなければ「時間がかかっています」を表示する。 */
export const SLOW_REPLY_HINT_MS = 20_000;

export type ChatErrorOptions = {
	/** 応答を待ちきれなかったときの案内文（どこで履歴を確かめるかが画面ごとに違う）。 */
	timeout: string;
	/** ほかの失敗は例外のメッセージを出す（音声入力・読み上げの失敗をそのまま案内するため）。 */
	detail?: boolean;
};

/** 送信・応答待ちの失敗を、画面に出す案内文にする。 */
export function describeChatError(
	cause: unknown,
	options: ChatErrorOptions,
): string {
	// 同期的な事前チェックの拒否（キー未設定 400 / 処理中 409 / レート制限 429）はサーバーの案内文をそのまま出す。
	if (
		cause instanceof ApiError &&
		cause.serverMessage &&
		[400, 409, 429].includes(cause.status ?? 0)
	)
		return cause.serverMessage;
	if (cause instanceof ChatReplyTimeoutError) return options.timeout;
	if (cause instanceof ApiError && cause.status === 401)
		return "ログインの有効期限が切れました。ログインし直してください。";
	if (options.detail && cause instanceof Error && cause.message)
		return cause.message;
	return "メッセージを送信できませんでした。";
}
