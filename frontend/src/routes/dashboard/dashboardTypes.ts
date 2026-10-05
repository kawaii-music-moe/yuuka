// ダッシュボードで消費するレスポンス形状のローカル型。
// StatusResponse / BotUsageResponse は汎用（{ [k]: unknown } / usage?）で他グループ所有のため、
// ここでフロントが参照する主要フィールドのみを型化する（サーバ settingsRoutes.ts /
// botAttributeRoutes.ts の実レスポンスに対応）。

/** GET /api/status の stats（旧 statusData.stats）。 */
export interface DashboardStats {
	tasks: number;
	pendingTasks: number;
	/** 優先度別未完了数（旧UI互換の 0=低/1=中/2=高 キー）。 */
	pendingPriorities: Record<number, number>;
	schedules: number;
	/** 直近5日のスケジュール数推移。 */
	scheduleTrend: number[];
	expenses: number;
	/** 過去5日の支出額推移。 */
	expenseTrend: number[];
}

/** GET /api/expenses（旧 expenseData）。 */
export interface DashboardExpense {
	id: number;
	amount: number;
	category: string;
	date: string;
}
export interface DashboardExpenseBreakdown {
	category: string;
	total: number;
	count?: number;
}

/** GET /api/bots/usage の日次系列（旧 data.series）。 */
export interface UsageSeriesPoint {
	date: string;
	requests: number;
	responses: number;
}
export interface UsageTotals {
	requests: number;
	responses: number;
}
export interface UsageRateLimits {
	userPerMinute?: number;
	userPerDay?: number;
	guildPerDay?: number;
}
/** GET /api/bots/usage レスポンス全体。 */
export interface UsageDashboardData {
	days?: number;
	series?: UsageSeriesPoint[];
	totals?: UsageTotals;
	rate_limits?: UsageRateLimits;
}
