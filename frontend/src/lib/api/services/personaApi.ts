// personaApi — src/server/routes/personaRoutes.ts に対応。
//
// スコープ（ハンドラの botId 読み取りに実挙動を合わせる）:
//   - list / save / delete / publish / activate / import は botId を読み、共有Botでは
//     オーナー名前空間へ正規化（owner-canonical・共同編集）する → scope:'bot'。
//     ※ 共有Bot上で「一覧はオーナーの集合／編集・作成・削除は自分の集合」に割れないよう、
//       ペルソナを指す全ての読み書きを bot-scoped に揃える（system_default/所有Botでは自分＝従来どおり）。
//   - marketplace（公開ペルソナの横断閲覧）は owner 非スコープ → scope:'user'。
import { api } from "../client";
import type {
	PersonasResponse,
	PersonaMarketplaceResponse,
	PersonaMarketplaceDetailResponse,
	ApiResponse,
} from "../types";

const USER = { scope: "user" } as const;
const BOT = { scope: "bot" } as const;

export const personaApi = {
	/** GET /api/personas — 自分のペルソナ一覧＋適用中ID＋最大文字数（bot-scoped: active 判定に botId を読む） */
	list: () => api.get<PersonasResponse>("/api/personas", BOT),

	/** POST /api/personas/save（body は { id?, name, prompt }・共有Botはオーナー集合へ・bot-scoped） */
	save: (body: { id?: number; name: string; prompt: string }) =>
		api.post<ApiResponse>("/api/personas/save", body, BOT),
	/** POST /api/personas/delete（共有Botはオーナー集合から・bot-scoped） */
	delete: (id: number) => api.post<ApiResponse>("/api/personas/delete", { id }, BOT),

	/** POST /api/personas/activate — 適用中ペルソナ切替（bot-scoped。null でデフォルトへ戻す） */
	activate: (id: number | null) =>
		api.post<ApiResponse>("/api/personas/activate", { id }, BOT),

	/** POST /api/personas/publish — 公開/非公開の切替（共有Botはオーナー集合対象・bot-scoped） */
	publish: (id: number, isPublic: boolean) =>
		api.post<ApiResponse>("/api/personas/publish", { id, isPublic }, BOT),

	/** GET /api/personas/marketplace — 公開ペルソナ一覧 */
	marketplace: () =>
		api.get<PersonaMarketplaceResponse>("/api/personas/marketplace", USER),
	/** GET /api/personas/marketplace/:id — 公開ペルソナ全文（インポート判断用） */
	marketplaceDetail: (id: number) =>
		api.get<PersonaMarketplaceDetailResponse>(
			`/api/personas/marketplace/${id}`,
			USER,
		),
	/** POST /api/personas/import — 公開ペルソナを独立コピー（共有Botはオーナー集合へ追加・bot-scoped） */
	import: (id: number) => api.post<ApiResponse>("/api/personas/import", { id }, BOT),
};
