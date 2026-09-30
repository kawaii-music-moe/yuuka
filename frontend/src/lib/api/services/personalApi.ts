// personalApi — bot-scoped（scope:'bot'）。src/server/routes/personalRoutes.ts に対応。
// context-note / clipboard / contacts（personal タブは3本 fetch）。
// サーバは全ルート auth:"user" だが botId を body/query から読み per-bot にスコープするため、
// クライアントは scope:'bot' で botId を自動注入する（旧 app.js は fetch のグローバル
// モンキーパッチで botId を付与していた挙動と一致）。
import { api } from "../client";
import type {
	ApiResponse,
	ClipboardResponse,
	ContactsResponse,
	ContextNoteResponse,
} from "../types";

const BOT = { scope: "bot" } as const;

export const personalApi = {
	// ── コンテキストノート ──
	/** GET /api/context-note → { content, max_length } */
	getContextNote: () => api.get<ContextNoteResponse>("/api/context-note", BOT),
	/** POST /api/context-note（body は { content }） */
	saveContextNote: (content: string) =>
		api.post<ApiResponse>("/api/context-note", { content }, BOT),

	// ── クリップボード ──
	/** GET /api/clipboard → { entries } */
	clipboard: () => api.get<ClipboardResponse>("/api/clipboard", BOT),
	/** POST /api/clipboard/delete */
	deleteClipboard: (id: number) =>
		api.post<ApiResponse>("/api/clipboard/delete", { id }, BOT),

	// ── 連絡先 ──
	/** GET /api/contacts → { contacts } */
	contacts: () => api.get<ContactsResponse>("/api/contacts", BOT),
	/** POST /api/contacts/save（サーバは contactInfo キーを読む。tags は string[]） */
	saveContact: (body: {
		id?: number;
		name: string;
		birthday?: string;
		relationship?: string;
		contactInfo?: string;
		notes?: string;
		tags?: string[];
	}) => api.post<ApiResponse>("/api/contacts/save", body, BOT),
	/** POST /api/contacts/delete */
	deleteContact: (id: number) =>
		api.post<ApiResponse>("/api/contacts/delete", { id }, BOT),
};
