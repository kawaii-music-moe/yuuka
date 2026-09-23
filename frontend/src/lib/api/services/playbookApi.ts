// playbookApi — bot-scoped（scope:'bot'）。src/server/routes/playbookRoutes.ts に対応。
// サーバは auth:"user" だが body/query の botId を読んで per-bot にスコープするため
// scope:'bot'（client が botId を自動注入）。
import { api } from "../client";
import type {
	ApiResponse,
	PlaybookRunsResponse,
	PlaybookSchedulesResponse,
	PlaybooksResponse,
} from "../types";

const BOT = { scope: "bot" } as const;

export const playbookApi = {
	/** GET /api/playbooks（query で全文検索可） */
	list: (query?: { query?: string }) =>
		api.get<PlaybooksResponse>("/api/playbooks", {
			...BOT,
			query: query?.query ? { query: query.query } : undefined,
		}),
	/** POST /api/playbooks/save（name は識別子。title/steps 必須） */
	save: (body: {
		name: string;
		title: string;
		keywords?: string[];
		description?: string;
		steps: string;
	}) => api.post<ApiResponse>("/api/playbooks/save", body, BOT),
	/** POST /api/playbooks/delete（name で削除） */
	delete: (name: string) =>
		api.post<ApiResponse>("/api/playbooks/delete", { name }, BOT),

	// ── スケジュール（定期実行） ──
	/** GET /api/playbooks/schedules */
	schedules: () =>
		api.get<PlaybookSchedulesResponse>("/api/playbooks/schedules", BOT),
	/** POST /api/playbooks/schedules/save（playbookName + cronExpression） */
	saveSchedule: (body: {
		playbookName: string;
		cronExpression: string;
		description?: string;
		enabled?: boolean;
	}) => api.post<ApiResponse>("/api/playbooks/schedules/save", body, BOT),
	/** POST /api/playbooks/schedules/toggle */
	toggleSchedule: (body: { id: number; enabled: boolean }) =>
		api.post<ApiResponse>("/api/playbooks/schedules/toggle", body, BOT),
	/** POST /api/playbooks/schedules/delete */
	deleteSchedule: (id: number) =>
		api.post<ApiResponse>("/api/playbooks/schedules/delete", { id }, BOT),

	/** GET /api/playbooks/runs — 実行履歴（scheduleId で絞り込み可） */
	runs: (query?: { scheduleId?: number }) =>
		api.get<PlaybookRunsResponse>("/api/playbooks/runs", {
			...BOT,
			query: query?.scheduleId ? { scheduleId: query.scheduleId } : undefined,
		}),
};
