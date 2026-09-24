// mcpApi — user-scoped（scope:'user'）。src/server/routes/mcpRoutes.ts に対応。
// MCP サーバは v4 以降ユーザースコープ（bot_id 列は退役）。botId は付けない。
//
// 注意: /proxy/mcp/:id/mcp と /api/mcp-servers/:id/dashboard は sandboxed iframe 経由で
//   呼ばれ、SPA 本体の fetch 経路ではない（§5.6）。ここでは iframe URL のヘルパのみ提供。
//
// ★add の body は camelCase（ctx.body.endpointUrl / authCredential / requiresConfirmation / scope）。
import { api } from "../client";
import type { ApiResponse, McpServersResponse } from "../types";

const USER = { scope: "user" } as const;

export const mcpApi = {
	/** GET /api/mcp-servers — 登録済み MCP サーバ一覧（owner + system） */
	list: () => api.get<McpServersResponse>("/api/mcp-servers", USER),

	/** POST /api/mcp-servers/add（scope:"system" は Admin のみ） */
	add: (body: {
		name: string;
		endpointUrl: string;
		authCredential?: string;
		requiresConfirmation?: boolean;
		scope?: "user" | "system";
	}) => api.post<ApiResponse>("/api/mcp-servers/add", body, USER),

	/** POST /api/mcp-servers/refresh — ツールキャッシュ更新 */
	refresh: (id: number) =>
		api.post<ApiResponse>("/api/mcp-servers/refresh", { id }, USER),

	/** POST /api/mcp-servers/toggle — 有効/無効切替 */
	toggle: (body: { id: number; enabled: boolean }) =>
		api.post<ApiResponse>("/api/mcp-servers/toggle", body, USER),

	/** POST /api/mcp-servers/delete */
	delete: (id: number) =>
		api.post<ApiResponse>("/api/mcp-servers/delete", { id }, USER),

	/** GET /api/mcp-servers/:id/dashboard/status — ダッシュボード提供の有無 */
	dashboardStatus: (id: number) =>
		api.get<ApiResponse & { available?: boolean }>(
			`/api/mcp-servers/${id}/dashboard/status`,
			USER,
		),

	/** iframe src 用の URL ヘルパ（/api/mcp-servers/:id/dashboard は独自 CSP で返る） */
	dashboardUrl: (id: number) => `/api/mcp-servers/${id}/dashboard`,
};
