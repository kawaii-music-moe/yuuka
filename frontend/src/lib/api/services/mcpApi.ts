// mcpApi — user-scoped（scope:'user'）。crates/yuuka-mcp/src/routes.rs に対応。
// MCP サーバは v4 以降ユーザースコープ（bot_id 列は退役）。botId は付けない。
//
// OAuth: oauthStart が返す authorizationUrl へ画面ごと遷移する。認可サーバーから
//   /api/mcp-servers/oauth/callback に戻り、結果をクエリ（mcp_oauth / message）に付けて
//   /integrated/mcp へ戻ってくる。
// MCP Apps: appsResource で画面（HTML）を受け取り、サンドボックス iframe に渡す。画面からの
//   tools/call・resources/read は appsCall / appsRead でホスト（サーバー）が代わりに実行する。
//
// ★add の body は camelCase（ctx.body.endpointUrl / authCredential / requiresConfirmation / scope）。
import { api } from "../client";
import type {
	ApiResponse,
	McpAppResourceResponse,
	McpAppResultResponse,
	McpServersResponse,
} from "../types";

const USER = { scope: "user" } as const;

export const mcpApi = {
	/** GET /api/mcp-servers — 登録済み MCP サーバ一覧（owner + system）と MCP Apps のサンドボックス */
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

	/** POST /api/mcp-servers/:id/oauth/start — OAuth 2.1 の認可 URL を得る */
	oauthStart: (
		id: number,
		body: { clientId?: string; clientSecret?: string } = {},
	) =>
		api.post<ApiResponse<{ authorizationUrl: string }>>(
			`/api/mcp-servers/${id}/oauth/start`,
			body,
			USER,
		),

	/** POST /api/mcp-servers/:id/oauth/disconnect — 保存した OAuth の資格情報を消す */
	oauthDisconnect: (id: number) =>
		api.post<ApiResponse>(`/api/mcp-servers/${id}/oauth/disconnect`, {}, USER),

	/** POST /api/mcp-servers/:id/apps/resource — ツールの画面（HTML と _meta.ui）を取得 */
	appsResource: (id: number, tool: string) =>
		api.post<McpAppResourceResponse>(
			`/api/mcp-servers/${id}/apps/resource`,
			{ tool },
			USER,
		),

	/** POST /api/mcp-servers/:id/apps/resource — 画面からの resources/read（ReadResourceResult） */
	appsRead: (id: number, uri: string) =>
		api.post<McpAppResultResponse>(
			`/api/mcp-servers/${id}/apps/resource`,
			{ uri },
			USER,
		),

	/**
	 * POST /api/mcp-servers/:id/apps/call — tools/call（CallToolResult）。
	 * from: "host" は画面を開くときの呼び出し、"app" は画面の中からの呼び出し。
	 */
	appsCall: (
		id: number,
		body: {
			name: string;
			arguments?: Record<string, unknown>;
			from: "host" | "app";
		},
	) =>
		api.post<McpAppResultResponse>(
			`/api/mcp-servers/${id}/apps/call`,
			body,
			USER,
		),
};
