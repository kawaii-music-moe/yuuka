// taskApi — bot-scoped（scope:'bot'）。src/server/routes/todoRoutes.ts に対応。
import { api } from "../client";
import type {
	ApiResponse,
	TaskDetailResponse,
	TaskGanttResponse,
	TasksResponse,
} from "../types";

const BOT = { scope: "bot" } as const;

export const taskApi = {
	/** GET /api/tasks — 親タスク＋サブタスク一覧（status/tag フィルタ可） */
	list: (query?: { status?: string; tag?: string }) =>
		api.get<TasksResponse>("/api/tasks", { ...BOT, query }),

	/** GET /api/tasks/gantt — ガント用タスク（start_date/due_date 付き） */
	gantt: () => api.get<TaskGanttResponse>("/api/tasks/gantt", BOT),

	/** GET /api/tasks/someday — 期日なしタスク */
	someday: () => api.get<TasksResponse>("/api/tasks/someday", BOT),

	/** GET /api/tasks/detail?id= — 単一タスク詳細＋進捗ログ */
	detail: (id: number) =>
		api.get<TaskDetailResponse>("/api/tasks/detail", { ...BOT, query: { id } }),

	/** POST /api/tasks/add */
	add: (body: {
		title: string;
		description?: string;
		dueDate?: string;
		startDate?: string;
		priority?: "high" | "medium" | "low";
		tags?: string[];
		parentId?: number;
		repeatRule?: string;
		repeatUntil?: string;
		repeatCount?: number;
	}) => api.post<TaskDetailResponse>("/api/tasks/add", body, BOT),

	/** POST /api/tasks/update */
	update: (body: {
		id: number;
		title?: string;
		description?: string;
		dueDate?: string;
		startDate?: string;
		priority?: "high" | "medium" | "low" | null;
		status?: "open" | "done";
	}) => api.post<TaskDetailResponse>("/api/tasks/update", body, BOT),

	/** POST /api/tasks/progress — 手動進捗報告（0-100 + note） */
	progress: (body: { id: number; progress: number; note?: string }) =>
		api.post<ApiResponse>("/api/tasks/progress", body, BOT),

	/** POST /api/tasks/complete */
	complete: (id: number) =>
		api.post<ApiResponse>("/api/tasks/complete", { id }, BOT),

	/** POST /api/tasks/delete */
	delete: (id: number) =>
		api.post<ApiResponse>("/api/tasks/delete", { id }, BOT),
};
