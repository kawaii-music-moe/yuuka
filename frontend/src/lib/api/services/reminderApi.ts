// reminderApi — bot-scoped（scope:'bot'）。src/server/routes/reminderRoutes.ts に対応。
import { api } from "../client";
import type { ApiResponse, RemindersResponse } from "../types";

const BOT = { scope: "bot" } as const;

export const reminderApi = {
	/**
	 * GET /api/reminders — リマインダー一覧。
	 * all=true で送信済み・キャンセル済みも含める（サーバは ?all=1 を読む）。
	 */
	list: (opts?: { all?: boolean }) =>
		api.get<RemindersResponse>("/api/reminders", {
			...BOT,
			query: opts?.all ? { all: 1 } : undefined,
		}),

	/** POST /api/reminders/add */
	add: (body: {
		message: string;
		trigger_at: string;
		repeat_rule?: string;
		target_type?: "dm" | "channel";
		target_id?: string;
	}) => api.post<ApiResponse>("/api/reminders/add", body, BOT),

	/** POST /api/reminders/cancel（サーバは reminder_id を受ける） */
	cancel: (id: number) =>
		api.post<ApiResponse>("/api/reminders/cancel", { reminder_id: id }, BOT),
};
