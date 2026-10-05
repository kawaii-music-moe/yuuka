// リマインダー タブの純関数群（DOM 非依存）。旧 app.js fetchRemindersList の表示ロジックを移植。
import type { ReminderRecord, ReminderStatus } from "$lib/api/types";

/** ステータス → 日本語ラベル（旧: pending=待機中 / sent=送信済み / cancelled=キャンセル）。 */
export function reminderStatusLabel(status: ReminderStatus): string {
	switch (status) {
		case "pending":
			return "待機中";
		case "sent":
			return "送信済み";
		case "cancelled":
			return "キャンセル";
		default:
			return status;
	}
}

/** 送信先の表示テキスト（📢 チャンネル / 📩 DM）。 */
export function reminderTargetText(rem: ReminderRecord): string {
	if (rem.target_type === "channel") {
		return rem.target_id ? `📢 チャンネル: ${rem.target_id}` : "📢 チャンネル";
	}
	return "📩 DM";
}

/**
 * datetime-local の値（YYYY-MM-DDTHH:MM）を API 用（YYYY-MM-DD HH:MM）へ変換。
 * 旧: triggerRaw.replace("T", " ")。
 */
export function toTriggerAt(datetimeLocal: string): string {
	return datetimeLocal.replace("T", " ");
}
