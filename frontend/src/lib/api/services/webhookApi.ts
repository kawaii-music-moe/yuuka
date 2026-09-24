// webhookApi — user-scoped（scope:'user'）。src/server/routes/webhookRoutes.ts に対応。
// ハンドラは botId を読まない（ユーザースコープ）。/hook/:token は外部 Webhook 受信ルート
// で SPA 経路ではないため提供しない。
import { api } from "../client";
import type {
	ApiResponse,
	WebhookCreateResponse,
	WebhookDeliveriesResponse,
	WebhooksResponse,
} from "../types";

const USER = { scope: "user" } as const;

export const webhookApi = {
	/** GET /api/webhooks → { endpoints } */
	list: () => api.get<WebhooksResponse>("/api/webhooks", USER),

	/** POST /api/webhooks/create（secret は16文字以上必須） */
	create: (body: {
		name: string;
		secret?: string;
		notifyTargetType?: "dm" | "channel";
		notifyTargetId?: string;
		template?: string;
		filterKeyword?: string;
		createTodo?: boolean;
		createReminder?: boolean;
	}) => api.post<WebhookCreateResponse>("/api/webhooks/create", body, USER),

	/** POST /api/webhooks/update（部分更新。enabled トグル等） */
	update: (body: { id: number; [k: string]: unknown }) =>
		api.post<WebhookCreateResponse>("/api/webhooks/update", body, USER),

	/** POST /api/webhooks/delete */
	delete: (id: number) =>
		api.post<ApiResponse>("/api/webhooks/delete", { id }, USER),

	/** GET /api/webhooks/deliveries?endpointId= — 受信履歴（直近50件） */
	deliveries: (endpointId?: number) =>
		api.get<WebhookDeliveriesResponse>("/api/webhooks/deliveries", {
			...USER,
			query: endpointId ? { endpointId } : undefined,
		}),
};
