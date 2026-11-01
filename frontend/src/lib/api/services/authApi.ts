// authApi — user-scoped（scope:'user'）。ログインは OIDC プロバイダー（/api/auth/oidc/start へのページ遷移）で行う。
// /api/logout|me は botId 除外（プレフィックス一致）。
import { api } from "../client";
import type { ApiResponse, MeResponse } from "../types";

const USER = { scope: "user" } as const;

/** GET /api/auth/oidc/provider。`label` はログイン先の表示名（ログインできないときは null）。 */
export type AuthProviderResponse = ApiResponse<{
	enabled: boolean;
	label: string | null;
}>;

export const authApi = {
	/**
	 * GET /api/me — セッションの単一の真実。未ログインで 401。
	 * 起動時プローブは isBootstrap:true を渡し 401 でも /login へ遷移させない。
	 */
	me: (isBootstrap = false) =>
		api.get<MeResponse>("/api/me", { ...USER, isBootstrap }),

	/** GET /api/auth/oidc/provider — ログイン画面向け（未ログインで呼ぶ）。 */
	provider: () =>
		api.get<AuthProviderResponse>("/api/auth/oidc/provider", {
			...USER,
			isBootstrap: true,
		}),

	/** POST /api/logout */
	logout: () => api.post<ApiResponse>("/api/logout", {}, USER),
};
