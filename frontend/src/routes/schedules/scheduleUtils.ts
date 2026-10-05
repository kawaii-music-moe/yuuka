// スケジュール関連の純関数（DOM 非依存）。旧 app.js fetchSchedulesList / scheduleForm submit
// の日時整形ロジックを切り出したもの。

/**
 * 開始/終了日時の表示テキストを組み立てる（旧 fetchSchedulesList）。
 *   開始: 'YYYY-MM-DD HH:mm'（先頭16文字）
 *   終了があれば ' 〜 HH:mm'（終了の時刻部のみ）を付加。
 */
export function formatScheduleRange(
	startAt: string,
	endAt: string | null,
): string {
	const startClean = startAt.slice(0, 16);
	const endClean = endAt ? ` 〜 ${endAt.slice(11, 16)}` : "";
	return `${startClean}${endClean}`;
}

/**
 * datetime-local の値（'YYYY-MM-DDTHH:mm'）を DB 形式（'YYYY-MM-DD HH:mm:ss'）へ変換する
 * （旧 scheduleForm submit の `.replace("T", " ") + ":00"`）。空文字は undefined。
 */
export function toDbDatetime(local: string): string | undefined {
	if (!local) return undefined;
	return `${local.replace("T", " ")}:00`;
}
