// webhookApi — src/server/routes/webhookRoutes.ts に対応。
// エンドポイントは Bot ごとの設定（受信で作る通知・ToDo・リマインドの帰属 Bot）。一覧・作成・受信履歴は
// 選択中 Bot を botId で渡す（scope:'bot'）。更新・削除は id で本人のエンドポイントを特定する
// （scope:'user'）。/hook/:token は外部 Webhook 受信ルートで SPA 経路ではないため提供しない。
import { api } from "../client";
import type {
	ApiResponse,
	WebhookCreateResponse,
	WebhookDeliveriesResponse,
	WebhooksResponse,
} from "../types";

const USER = { scope: "user" } as const;
const BOT = { scope: "bot" } as const;

export const webhookApi = {
	/** GET /api/webhooks?botId= → { endpoints }（選択中 Bot の分） */
	list: () => api.get<WebhooksResponse>("/api/webhooks", BOT),

	/** POST /api/webhooks/create（secret は16文字以上必須・選択中 Bot へ帰属） */
	create: (body: {
		name: string;
		secret?: string;
		notifyTargetType?: "dm" | "channel";
		notifyTargetId?: string;
		template?: string;
		filterKeyword?: string;
		createTodo?: boolean;
		createReminder?: boolean;
	}) => api.post<WebhookCreateResponse>("/api/webhooks/create", body, BOT),

	/** POST /api/webhooks/update（部分更新。enabled トグル等） */
	update: (body: { id: number; [k: string]: unknown }) =>
		api.post<WebhookCreateResponse>("/api/webhooks/update", body, USER),

	/** POST /api/webhooks/delete */
	delete: (id: number) =>
		api.post<ApiResponse>("/api/webhooks/delete", { id }, USER),

	/** GET /api/webhooks/deliveries?botId=&endpointId= — 受信履歴（選択中 Bot の分・直近50件） */
	deliveries: (endpointId?: number) =>
		api.get<WebhookDeliveriesResponse>("/api/webhooks/deliveries", {
			...BOT,
			query: endpointId ? { endpointId } : undefined,
		}),
};
