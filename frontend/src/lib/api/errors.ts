// API 呼び出しの失敗を表すエラーと、画面に出す文言の取り出し。
// 他モジュールに依存しない（stores/toast からも import できるようにするため）。

export class ApiError extends Error {
	constructor(
		public status: number,
		message: string,
	) {
		super(message);
		this.name = "ApiError";
	}
}

/** 既定の失敗文言。サーバーが理由を返さなかったときに出す。 */
export const DEFAULT_ERROR_MESSAGE = "エラーが発生しました";

/**
 * 画面に出すエラー文言。ApiError ならサーバーの message を、それ以外（通信断・
 * 想定外の例外）なら fallback を返す。
 */
export function errorMessage(
	e: unknown,
	fallback: string = DEFAULT_ERROR_MESSAGE,
): string {
	return e instanceof ApiError ? e.message : fallback;
}
