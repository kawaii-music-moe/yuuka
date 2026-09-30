// ダッシュボードの非チャート系純関数（吹き出し文言・緊急リスト整形）。
import type { ScheduleRecord, TodoWithSubtasks } from "$lib/api/types";
import { formatMonthDay, yen } from "./dashboardCharts";
import type { UsageSeriesPoint, UsageTotals } from "./dashboardTypes";

/** 秘書モードの吹き出し文言（旧 updateYuukaSpeechBubble）。 */
export function secretaryBubble(
	totalExpenses: number,
	pendingTasks: number,
): string {
	if (totalExpenses > 30000) {
		return `今月の支出が ${yen(totalExpenses)} に達しています。予算を確認してみましょう。`;
	}
	if (pendingTasks > 5) {
		return `未完了タスクが ${pendingTasks} 件あります。優先度の高いものから片付けていきましょう。`;
	}
	return "お疲れ様です。タスク・予定・家計の管理をサポートします。何かあればDiscordでお声がけください。";
}

/** 汎用モードの吹き出し文言（旧 updateAssistantSpeechBubble）。 */
export function assistantBubble(
	series: UsageSeriesPoint[],
	totals: UsageTotals,
	days: number,
): string {
	const today = series.length ? series[series.length - 1].requests : 0;
	const total = totals.requests || 0;
	if (total === 0) {
		return "まだAPIの利用がありません。Discordサーバーでこのアシスタントにメンションして話しかけてみましょう。";
	}
	return `直近${days}日間で ${total.toLocaleString()} 件のリクエストを処理しました（本日 ${today.toLocaleString()} 件）。下のチャートでAPI使用量の推移を確認できます。`;
}

/** ピーク日サマリ表示。 */
export function peakDateLabel(peakDate: string): string {
	return peakDate ? `${formatMonthDay(peakDate)} が最多` : "データなし";
}

// ── 緊急リスト（旧 renderUrgentDashboardList） ──

export interface UrgentItem {
	icon: string;
	label: string;
	badge: string;
	badgeClass: string;
}

/** 今日の予定（先頭2件）＋未消化タスク（先頭3件）を緊急リスト行へ整形。 */
export function buildUrgentItems(
	schedules: ScheduleRecord[],
	tasks: TodoWithSubtasks[],
): UrgentItem[] {
	const items: UrgentItem[] = [];

	for (const sched of schedules.slice(0, 2)) {
		items.push({
			icon: "calendar_today",
			label: ` [今日の予定] ${sched.title}`,
			badge: sched.start_at.slice(11, 16),
			badgeClass: "badge-urgent",
		});
	}

	for (const task of tasks.slice(0, 3)) {
		items.push({
			icon: "checklist",
			label: ` [未消化タスク] ${task.title}`,
			badge: task.priority === "high" ? "優先: 高" : "優先: 普通",
			badgeClass: "badge-normal",
		});
	}

	return items;
}

/** 最大単発支出。 */
export function highestExpense(
	expenses: { amount: number }[] | undefined,
): number {
	if (!expenses || expenses.length === 0) return 0;
	return Math.max(...expenses.map((e) => e.amount));
}

/** 主要カテゴリ（breakdown 先頭）。 */
export function highestCategory(
	breakdown: { category: string }[] | undefined,
): string {
	return breakdown && breakdown.length > 0 ? breakdown[0].category : "なし";
}

/** 30日日次平均。 */
export function averageExpense(
	total: number,
	expenses: unknown[] | undefined,
): number {
	return expenses && expenses.length > 0 ? Math.round(total / 30) : 0;
}

/** ローカル日付 'YYYY-MM-DD'。 */
export function todayIso(): string {
	return new Date().toISOString().slice(0, 10);
}
