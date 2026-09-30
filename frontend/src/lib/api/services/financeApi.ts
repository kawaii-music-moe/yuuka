// financeApi — bot-scoped（scope:'bot'）。src/server/routes/financeRoutes.ts に対応。
import { api } from "../client";
import type {
	ApiResponse,
	BudgetLimitsResponse,
	ExpensesResponse,
	PlannedPaymentsResponse,
} from "../types";

const BOT = { scope: "bot" } as const;

export const financeApi = {
	/**
	 * GET /api/expenses — 収支一覧＋集計（total/incomeTotal/breakdown/trend）。
	 * 月フィルタは year/month（数値）。未指定ならサーバが当月を採用。
	 */
	list: (query?: { year?: number; month?: number }) =>
		api.get<ExpensesResponse>("/api/expenses", { ...BOT, query }),

	/** POST /api/expenses/add — サーバは description を memo として保存する。 */
	add: (body: {
		type: "income" | "expense";
		amount: number;
		category: string;
		description?: string;
		date?: string;
		time?: string;
	}) => api.post<ApiResponse>("/api/expenses/add", body, BOT),

	/**
	 * POST /api/expenses/upload-receipt — レシートOCR。
	 * サーバは JSON { imageBase64, mimeType, additionalText } を読む（multipart 非対応）。
	 * botId は client が JSON body へ自動注入する（scope:'bot'）。
	 */
	uploadReceipt: (body: {
		imageBase64: string;
		mimeType: string;
		additionalText?: string;
	}) =>
		api.post<ApiResponse<{ response: string }>>(
			"/api/expenses/upload-receipt",
			body,
			BOT,
		),

	// ── 予算上限 ──
	/** GET /api/expenses/budget-limits */
	budgetLimits: () =>
		api.get<BudgetLimitsResponse>("/api/expenses/budget-limits", BOT),
	/** POST /api/expenses/budget-limits — サーバは limitAmount（camelCase）を読む。 */
	saveBudgetLimit: (body: { category: string; limitAmount: number }) =>
		api.post<ApiResponse>("/api/expenses/budget-limits", body, BOT),
	/** POST /api/expenses/budget-limits/delete */
	deleteBudgetLimit: (category: string) =>
		api.post<ApiResponse>(
			"/api/expenses/budget-limits/delete",
			{ category },
			BOT,
		),

	// ── 予定支払（plannedPayment） ──
	/** GET /api/expenses/plans（既定 pending のみ。includePaid=true で全件） */
	plans: (includePaid = false) =>
		api.get<PlannedPaymentsResponse>("/api/expenses/plans", {
			...BOT,
			query: includePaid ? { includePaid: "true" } : undefined,
		}),
	/** POST /api/expenses/plans/add — サーバは plannedDate/description を読む。 */
	addPlan: (body: {
		title: string;
		amount: number;
		category: string;
		plannedDate: string;
		description?: string;
	}) => api.post<ApiResponse>("/api/expenses/plans/add", body, BOT),
	/** POST /api/expenses/plans/pay */
	payPlan: (id: number) =>
		api.post<ApiResponse>("/api/expenses/plans/pay", { id }, BOT),
	/** POST /api/expenses/plans/delete */
	deletePlan: (id: number) =>
		api.post<ApiResponse>("/api/expenses/plans/delete", { id }, BOT),
};
