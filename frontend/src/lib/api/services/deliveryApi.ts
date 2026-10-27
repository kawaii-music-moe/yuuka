// deliveryApi — 定期報告（/api/periodic-reports）。bot-scoped（scope:'bot'）。
// ハンドラは body.botId ?? query botId を読む。他人の定期報告は 404。
import { api } from "../client";
import type {
	ApiResponse,
	PeriodicReportInput,
	PeriodicReportResponse,
	PeriodicReportsResponse,
} from "../types";

const BOT = { scope: "bot" } as const;

export const deliveryApi = {
	/** GET /api/periodic-reports */
	list: () => api.get<PeriodicReportsResponse>("/api/periodic-reports", BOT),
	/** POST /api/periodic-reports/create */
	create: (body: PeriodicReportInput) =>
		api.post<PeriodicReportResponse>("/api/periodic-reports/create", body, BOT),
	/** POST /api/periodic-reports/update（部分更新。有効/無効の切替もこれ） */
	update: (id: number, body: PeriodicReportInput) =>
		api.post<PeriodicReportResponse>(
			"/api/periodic-reports/update",
			{ id, ...body },
			BOT,
		),
	/** POST /api/periodic-reports/delete */
	delete: (id: number) =>
		api.post<ApiResponse>("/api/periodic-reports/delete", { id }, BOT),
	/** POST /api/periodic-reports/test — テスト配信 */
	test: (id: number) =>
		api.post<ApiResponse>("/api/periodic-reports/test", { id }, BOT),
};
