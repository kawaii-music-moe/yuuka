// authApi — user-scoped（scope:'user'）。ログインは sk-account（/api/auth/sk/start へのページ遷移）で行う。
// /api/logout|me は botId 除外（プレフィックス一致）。
import { api } from "../client";
import type { ApiResponse, MeResponse } from "../types";

const USER = { scope: "user" } as const;

export const authApi = {
	/**
	 * GET /api/me — セッションの単一の真実。未ログインで 401。
	 * 起動時プローブは isBootstrap:true を渡し 401 でも /login へ遷移させない。
	 */
	me: (isBootstrap = false) =>
		api.get<MeResponse>("/api/me", { ...USER, isBootstrap }),

	/** POST /api/logout */
	logout: () => api.post<ApiResponse>("/api/logout", {}, USER),
};
