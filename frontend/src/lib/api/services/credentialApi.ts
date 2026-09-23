// credentialApi — src/server/routes/credentialRoutes.ts に対応。
//
// GET /api/credentials は botId を読む（listCredentialNamesForBot）→ scope:'bot'。
// register/delete はユーザー所有の全 bot へ付与するため botId を読まない → scope:'user'。
import { api } from "../client";
import type { ApiResponse, CredentialsResponse } from "../types";

const USER = { scope: "user" } as const;
const BOT = { scope: "bot" } as const;

export const credentialApi = {
	/** GET /api/credentials — 認証情報一覧（bot-scoped: 付与状況を botId で判定） */
	list: () => api.get<CredentialsResponse>("/api/credentials", BOT),

	/** POST /api/credentials/register — 認証情報登録（全所有 Bot へ付与） */
	register: (body: {
		serviceName: string;
		credential: string;
		[k: string]: unknown;
	}) => api.post<ApiResponse>("/api/credentials/register", body, USER),

	/** POST /api/credentials/delete */
	delete: (serviceName: string) =>
		api.post<ApiResponse>("/api/credentials/delete", { serviceName }, USER),
};
