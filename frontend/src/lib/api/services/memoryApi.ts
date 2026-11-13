// memoryApi — 会話記憶（シナプス）の一覧・削除。crates/yuuka-orchestrator/src/memory_routes.rs に対応。
// 記憶は利用者 × Bot のスコープだが、「あなたの設定」は Bot を選ばずに開くため、カード側で選んだ
// Bot の id を明示的に query に付ける（activeBot からの自動注入＝scope:'bot' は使わない）。
import { api } from "../client";
import type { ApiResponse } from "../types";

const USER = { scope: "user" } as const;

/** 利用者に見せる記憶 1 件（kept）。内部スコープ列は返らない。 */
export interface MemoryItem {
	id: number;
	/** 評価器が書いた事実文（評価前からある行は発話の生テキスト） */
	content: string;
	/** 評価器の分類（preference / fact / schedule 等。未評価は null） */
	kind: string | null;
	/** long / medium / short */
	durability: string;
	strength: number;
	use_count: number;
	useful_count: number;
	last_used_at: string | null;
	created_at: string;
}

export type MemoryListResponse = ApiResponse<{
	memories: MemoryItem[];
	/** 評価待ち（candidate）の件数。内容は返らない。 */
	pendingCount: number;
}>;

export const memoryApi = {
	/** GET /api/memories?botId= — kept を強度の高い順に */
	list: (botId: string) =>
		api.get<MemoryListResponse>("/api/memories", {
			...USER,
			query: { botId },
		}),
	/** DELETE /api/memories/{id}?botId= — 本人 × Bot の kept を 1 件消す（該当無は success:false） */
	remove: (botId: string, id: number) =>
		api.del<ApiResponse>(`/api/memories/${id}`, { ...USER, query: { botId } }),
};
