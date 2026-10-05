// Webhook タブの純関数（DOM 非依存）。
import type { WebhookDeliveryRecord } from "$lib/api/types";

/** 配信ステータス → 表示ラベル（旧 status ラベルマップ）。 */
export function deliveryStatusLabel(
	status: WebhookDeliveryRecord["status"],
): string {
	switch (status) {
		case "received":
			return "受信";
		case "notified":
			return "通知済み";
		case "filtered":
			return "フィルタ済み";
		case "failed":
			return "失敗";
		default:
			return status;
	}
}

/** 配信ステータス → Badge のトーンクラス。 */
export function deliveryStatusTone(
	status: WebhookDeliveryRecord["status"],
): string {
	switch (status) {
		case "notified":
			return "status-active";
		case "failed":
			return "status-suspended";
		case "filtered":
			return "status-default";
		default:
			return "";
	}
}
