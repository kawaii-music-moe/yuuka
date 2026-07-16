// ─────────────────────────────────────────────────────────────────────────────
// BotTasks 共通ユーティリティ（旧 app.js の tasks 純関数群を移植）
// - parseTaskTags / fmtTaskDate / PRIORITY_LABELS / 進捗算出
// - ガント正規化（toGanttRow / parseDateMs / startOfTodayMs / DAY_MS）
// §11.4: DOM は作らない。純粋なデータ変換のみ。表示は各コンポーネントが担う。
// ─────────────────────────────────────────────────────────────────────────────

import type { TodoWithSubtasks } from "$lib/api/types";

/** 旧 app.js:2241 PRIORITY_LABELS */
export const PRIORITY_LABELS: Record<string, string> = {
	high: "高",
	medium: "中",
	low: "低",
};

export function priorityLabel(priority: string | null | undefined): string {
	return (priority && PRIORITY_LABELS[priority]) || "—";
}

/** tags（JSON文字列）を配列へ（旧 parseTaskTags）。不正な JSON は空配列。 */
export function parseTaskTags(raw: string | null | undefined): string[] {
	try {
		const t = JSON.parse(raw || "[]");
		return Array.isArray(t) ? (t as string[]) : [];
	} catch {
		return [];
	}
}

/** ISO文字列の表示整形（旧 fmtTaskDate）。日付のみ→そのまま、日時→分まで。 */
export function fmtTaskDate(s: string | null | undefined): string {
	if (!s) return "";
	return s.includes("T") ? s.slice(0, 16).replace("T", " ") : s;
}

/**
 * カード/行に表示する進捗率（旧 buildTaskCard/buildSubtaskRow の percent 算出）。
 * effective_progress を最優先 → done は100 → それ以外は progress。
 */
export function displayPercent(task: TodoWithSubtasks): number {
	if (typeof task.effective_progress === "number") return task.effective_progress;
	if (task.status === "done") return 100;
	return task.progress || 0;
}

/**
 * 完了チェックボックスを無効化すべきか（旧仕様）。
 * サブタスクを持ち、かつ算出進捗が100%未満なら手動完了不可。
 */
export function completionDisabled(task: TodoWithSubtasks): boolean {
	const subs = task.subtasks || [];
	return subs.length > 0 && displayPercent(task) < 100;
}

/** 子孫総数（旧 countDescendants。折りたたみトグルの「N件」表示用）。 */
export function countDescendants(nodes: TodoWithSubtasks[]): number {
	return nodes.reduce((n, c) => n + 1 + countDescendants(c.subtasks || []), 0);
}

// ─── ガント（旧 parseDateMs / startOfTodayMs / DAY_MS / toGanttRow） ────────────

export const DAY_MS = 24 * 60 * 60 * 1000;

/** 日付文字列→ms（旧 parseDateMs）。YYYY-MM-DD はローカル 0時扱い。 */
export function parseDateMs(s: string | null | undefined): number | null {
	if (!s) return null;
	if (/^\d{4}-\d{2}-\d{2}$/.test(s)) {
		const [y, m, d] = s.split("-").map(Number);
		return new Date(y, m - 1, d).getTime();
	}
	const t = new Date(s).getTime();
	return Number.isNaN(t) ? null : t;
}

/** 今日の0時(ms)（旧 startOfTodayMs）。 */
export function startOfTodayMs(): number {
	const n = new Date();
	return new Date(n.getFullYear(), n.getMonth(), n.getDate()).getTime();
}

export interface GanttRow {
	label: string;
	range: [number, number];
	color: string;
	progress: number;
}

/**
 * 1タスクをガント1行へ正規化（旧 toGanttRow）。日付が全く無ければ null。
 * 始端・終端補完: 期限のみ→単日 / 開始のみ→今日まで。最低1日幅を確保。
 */
export function toGanttRow(
	task: TodoWithSubtasks,
	indent: boolean,
): GanttRow | null {
	const startMs = parseDateMs(task.start_date);
	const dueMs = parseDateMs(task.due_date);
	if (startMs == null && dueMs == null) return null;
	const today = startOfTodayMs();
	const s = startMs != null ? startMs : (dueMs as number);
	let e = dueMs != null ? dueMs : Math.max(startMs as number, today);
	if (e <= s) e = s + DAY_MS;
	const percent = displayPercent(task);
	const overdue = task.status !== "done" && dueMs != null && dueMs < today;
	const color =
		task.status === "done"
			? "rgba(120,120,128,0.55)"
			: overdue
				? "rgba(237,66,69,0.65)"
				: "rgba(187,134,252,0.55)";
	return {
		label: `${indent ? "↳ " : ""}${task.title}`,
		range: [s, e],
		color,
		progress: percent,
	};
}

/** 親→サブタスクの順で行配列を構築（旧 renderGantt のループ）。 */
export function buildGanttRows(tasks: TodoWithSubtasks[]): GanttRow[] {
	const rows: GanttRow[] = [];
	for (const t of tasks) {
		const parentRow = toGanttRow(t, false);
		if (parentRow) rows.push(parentRow);
		for (const sub of t.subtasks || []) {
			const r = toGanttRow(sub, true);
			if (r) rows.push(r);
		}
	}
	return rows;
}
