// deliveryApi — bot-scoped（scope:'bot'）。src/server/routes/deliveryRoutes.ts に対応。
// briefing-config / report-configs。ハンドラは ctx.body.botId ?? query botId を読む（bot-scoped）。
import { api } from "../client";
import type {
	ApiResponse,
	BriefingConfigResponse,
	ReportConfigsResponse,
} from "../types";

const BOT = { scope: "bot" } as const;

export const deliveryApi = {
	// ── ブリーフィング設定 ──
	/** GET /api/briefing-config */
	getBriefingConfig: () =>
		api.get<BriefingConfigResponse>("/api/briefing-config", BOT),
	/** POST /api/briefing-config */
	saveBriefingConfig: (body: Record<string, unknown>) =>
		api.post<ApiResponse>("/api/briefing-config", body, BOT),
	/** POST /api/briefing/test — テスト配信 */
	testBriefing: () => api.post<ApiResponse>("/api/briefing/test", {}, BOT),

	// ── レポート設定 ──
	/** GET /api/report-configs */
	reportConfigs: () =>
		api.get<ReportConfigsResponse>("/api/report-configs", BOT),
	/** POST /api/report-configs */
	saveReportConfig: (body: Record<string, unknown>) =>
		api.post<ApiResponse>("/api/report-configs", body, BOT),
	/** POST /api/report-configs/test */
	testReport: (body: Record<string, unknown>) =>
		api.post<ApiResponse>("/api/report-configs/test", body, BOT),
};
