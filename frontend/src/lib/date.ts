// 画面間で共有する日付の小関数（DOM 非依存）。

/**
 * 今日の日付 'YYYY-MM-DD'（旧 new Date().toISOString().slice(0,10)）。
 * toISOString は UTC なので、JST の 0〜9 時は前日の日付になる（旧実装どおり）。
 */
export function todayIso(): string {
	return new Date().toISOString().slice(0, 10);
}
