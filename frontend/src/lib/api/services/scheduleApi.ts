// scheduleApi — bot-scoped（scope:'bot'）。src/server/routes/scheduleRoutes.ts に対応。
import { api } from "../client";
import type { ApiResponse, SchedulesResponse } from "../types";

const BOT = { scope: "bot" } as const;

export const scheduleApi = {
	/** GET /api/schedules?days=N — 直近 N 日間の予定一覧（既定7日） */
	list: (days = 7) =>
		api.get<SchedulesResponse>("/api/schedules", { ...BOT, query: { days } }),

	/**
	 * POST /api/schedules/add
	 * サーバは camelCase（startAt/endAt/remindBeforeMinutes）を読む。
	 * start_at/end_at は 'YYYY-MM-DD HH:mm:ss' 形式（旧 app.js 互換）。
	 */
	add: (body: {
		title: string;
		description?: string;
		startAt: string;
		endAt?: string;
		remindBeforeMinutes?: number;
	}) => api.post<ApiResponse>("/api/schedules/add", body, BOT),

	/** POST /api/schedules/delete */
	delete: (id: number) =>
		api.post<ApiResponse>("/api/schedules/delete", { id }, BOT),
};
