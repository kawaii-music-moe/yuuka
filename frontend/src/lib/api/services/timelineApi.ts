// timelineApi — bot-scoped（scope:'bot'）。src/server/routes/timelineRoutes.ts に対応。
import { api } from "../client";
import type {
	ApiResponse,
	PlanBlockType,
	RecordType,
	TimelineDayResponse,
} from "../types";

const BOT = { scope: "bot" } as const;

/** 計画ブロック 追加/更新の共通ペイロード（サーバは camelCase を受ける）。 */
export interface PlanBlockPayload {
	date: string;
	type: PlanBlockType;
	title: string;
	startTime?: string;
	endTime?: string;
	description?: string;
	transitFrom?: string;
	transitTo?: string;
	transitLine?: string;
}

/** 記録 追加ペイロード（memo/expense/task_done/location）。media は uploadMedia を使う。 */
export interface RecordPayload {
	date: string;
	type: RecordType;
	title?: string;
	content?: string;
	location?: string;
	amount?: number;
	category?: string;
}

/** メディア（写真・動画）アップロードのペイロード（サーバは base64 JSON を受ける）。 */
export interface MediaPayload {
	date: string;
	base64: string;
	mimeType: string;
	title?: string;
	location?: string;
}

export const timelineApi = {
	/** GET /api/timeline/day?date=YYYY-MM-DD — その日の blocks + records */
	day: (date: string) =>
		api.get<TimelineDayResponse>("/api/timeline/day", {
			...BOT,
			query: { date },
		}),

	/** POST /api/timeline/plan — 予定ブロック追加 */
	addPlan: (body: PlanBlockPayload) =>
		api.post<ApiResponse>("/api/timeline/plan", body, BOT),

	/** POST /api/timeline/plan/update（id 付き） */
	updatePlan: (body: PlanBlockPayload & { id: number }) =>
		api.post<ApiResponse>("/api/timeline/plan/update", body, BOT),

	/** POST /api/timeline/plan/delete */
	deletePlan: (id: number) =>
		api.post<ApiResponse>("/api/timeline/plan/delete", { id }, BOT),

	/** POST /api/timeline/record — 記録（memo/expense/task_done/location）追加 */
	addRecord: (body: RecordPayload) =>
		api.post<ApiResponse>("/api/timeline/record", body, BOT),

	/** POST /api/timeline/record/delete */
	deleteRecord: (id: number) =>
		api.post<ApiResponse>("/api/timeline/record/delete", { id }, BOT),

	/**
	 * POST /api/timeline/media — メディアアップロード。
	 * サーバは multipart ではなく base64 JSON（{ date, base64, mimeType, title?, location? }）を受ける。
	 */
	uploadMedia: (body: MediaPayload) =>
		api.post<ApiResponse>("/api/timeline/media", body, BOT),

	/** GET /api/timeline/media/:filename の URL を組み立てるヘルパ（画像 src 用） */
	mediaUrl: (filename: string) =>
		`/api/timeline/media/${encodeURIComponent(filename)}`,
};
