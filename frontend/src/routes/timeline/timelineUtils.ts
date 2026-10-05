// タイムライン タブの純関数群（DOM 非依存）。旧 app.js タイムライン群を移植。
import type { PlanBlockType, RecordType } from "$lib/api/types";

const DAY_NAMES = ["日", "月", "火", "水", "木", "金", "土"];

/** ローカル日付 'YYYY-MM-DD' を取得。 */
export function todayIso(): string {
	return new Date().toISOString().slice(0, 10);
}

/** 'YYYY-MM-DD' → 'YYYY-MM-DD (曜)'（旧 tlFmtDate）。 */
export function fmtTimelineDate(iso: string): string {
	const d = new Date(iso + "T00:00:00");
	return `${iso} (${DAY_NAMES[d.getDay()]})`;
}

/** 日付を delta 日ずらして 'YYYY-MM-DD' を返す（旧 tlShiftDay のロジック部分）。 */
export function shiftDay(iso: string, delta: number): string {
	const d = new Date(iso + "T00:00:00");
	d.setDate(d.getDate() + delta);
	return d.toISOString().slice(0, 10);
}

// タイプ → アイコン/ラベル（旧 PLAN_TYPE_ICON/LABEL, RECORD_TYPE_ICON/LABEL）
export const PLAN_TYPE_ICON: Record<PlanBlockType, string> = {
	task: "checklist",
	transit: "train",
	event: "event",
	free: "self_improvement",
};
export const PLAN_TYPE_LABEL: Record<PlanBlockType, string> = {
	task: "タスク",
	transit: "移動",
	event: "イベント",
	free: "フリー",
};
export const RECORD_TYPE_ICON: Record<RecordType, string> = {
	memo: "edit_note",
	expense: "payments",
	task_done: "check_circle",
	media: "photo_camera",
	location: "place",
};
export const RECORD_TYPE_LABEL: Record<RecordType, string> = {
	memo: "メモ",
	expense: "支出",
	task_done: "完了",
	media: "メディア",
	location: "場所",
};

export function planTypeIcon(type: PlanBlockType): string {
	return PLAN_TYPE_ICON[type] ?? "event";
}
export function recordTypeIcon(type: RecordType): string {
	return RECORD_TYPE_ICON[type] ?? "edit_note";
}
export function recordTypeLabel(type: RecordType): string {
	return RECORD_TYPE_LABEL[type] ?? type;
}

/** 計画ブロックの時刻レンジ表示（'HH:MM 〜 HH:MM' / 片方のみ / 空）。 */
export function planTimeRange(
	start: string | null,
	end: string | null,
): string {
	return [start, end].filter(Boolean).join(" 〜 ");
}

/** 記録の時刻（recorded_at の HH:MM 部分）。 */
export function recordTime(recordedAt: string | null): string {
	return recordedAt ? recordedAt.slice(11, 16) : "";
}

/** 移動情報の 'A → B (路線)' 表示。 */
export function transitText(
	from: string | null,
	to: string | null,
	line: string | null,
): string {
	const route = [from, to].filter(Boolean).join(" → ");
	return route + (line ? ` (${line})` : "");
}

/**
 * File を base64（データ URI ヘッダ除去済み）へ変換する。
 * 旧 record モーダルの FileReader.readAsDataURL → split(",")[1] を Promise 化。
 */
export function fileToBase64(file: File): Promise<string> {
	return new Promise((resolve, reject) => {
		const reader = new FileReader();
		reader.onload = () => {
			const result = reader.result;
			if (typeof result !== "string") {
				reject(new Error("ファイルの読み込みに失敗しました。"));
				return;
			}
			resolve(result.split(",")[1] ?? "");
		};
		reader.onerror = () => reject(reader.error ?? new Error("読み込みエラー"));
		reader.readAsDataURL(file);
	});
}
