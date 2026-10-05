// Playbook タブの純関数（DOM 非依存）。旧 app.js の run 表示ロジックを切り出し。
import type { PlaybookRunRecord } from "$lib/api/types";

/** 実行時間を「N秒」/「N分N秒」で表す（旧 duration 表示）。両端が揃わなければ空文字。 */
export function formatRunDuration(run: PlaybookRunRecord): string {
	if (!run.started_at || !run.finished_at) return "";
	const start = Date.parse(run.started_at);
	const end = Date.parse(run.finished_at);
	if (Number.isNaN(start) || Number.isNaN(end) || end < start) return "";
	const sec = Math.round((end - start) / 1000);
	if (sec < 60) return `${sec}秒`;
	const m = Math.floor(sec / 60);
	const s = sec % 60;
	return `${m}分${s}秒`;
}

/** run.status → Material Symbols アイコン名（旧 status アイコンマップ）。 */
export function runStatusIcon(status: PlaybookRunRecord["status"]): string {
	switch (status) {
		case "success":
			return "check_circle";
		case "failed":
			return "cancel";
		default:
			return "pending";
	}
}

/** run.status → 表示ラベル。 */
export function runStatusLabel(status: PlaybookRunRecord["status"]): string {
	switch (status) {
		case "success":
			return "成功";
		case "failed":
			return "失敗";
		default:
			return "実行中";
	}
}
