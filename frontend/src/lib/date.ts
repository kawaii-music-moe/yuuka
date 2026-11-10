// 画面間で共有する日付の小関数（DOM 非依存）。

/** ブラウザのローカル暦での今日の日付 'YYYY-MM-DD'。 */
export function todayIso(): string {
	const now = new Date();
	const year = String(now.getFullYear()).padStart(4, "0");
	const month = String(now.getMonth() + 1).padStart(2, "0");
	const day = String(now.getDate()).padStart(2, "0");
	return `${year}-${month}-${day}`;
}
