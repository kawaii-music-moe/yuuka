export type HealthStatus = { status: 'ok' | 'error'; service: string; checkedAt: string }
export type AgentSettings = {
  googleConnected: boolean
  googleAccount?: string
  model: string
  persona: string
}
export type SharedNote = { id: string; title: string; body: string; updatedAt: string }
export type Todo = { id: string; title: string; dueDate?: string; completed: boolean; list: string }
/** 予定の追加。日時はローカル暦の `YYYY-MM-DDTHH:MM`（`datetime-local` の値）。 */
export type NewCalendarEvent = { title: string; startsAt: string; endsAt?: string; description?: string }
export type CalendarEvent = { id: string; title: string; startsAt: string; endsAt: string; calendar: string; calendarName?: string; color: string }
export type Transaction = { id: string; date: string; category: string; description: string; amount: number; kind: 'income' | 'expense' }
/** レシートから読み取った取引の下書き。読み取れなかった欄は null。 */
export type ReceiptDraft = { date: string | null; kind: 'income' | 'expense'; category: string; description: string; amount: number | null }
export type FinanceSummary = { income: number; expense: number; balance: number; month: string }
export type ChatReference = {
  type: 'todo' | 'calendar' | 'finance' | 'note'
  id?: string
  title: string
  description: string
  href: string
  meta?: string
}
/** リッチ返信の埋め込みフィールド（Discord Embed 相当）。 */
export type ChatEmbedField = { name: string; value: string; inline: boolean }
/**
 * リッチ返信の埋め込み（`GET /api/client/chat/messages` の `embeds`）。
 * `color` は 0xRRGGBB の数値。すべて省略可能（サーバーは値が無い項目を送らない）。
 */
export type ChatEmbed = {
  title?: string
  description?: string
  color?: number
  fields?: ChatEmbedField[]
  footer?: string
}
/**
 * リッチ返信のファイル添付（`files`）。実体は認証必須・所有者スコープの `url`
 * （`GET /api/client/chat/attachments/:id`）から取得する。
 */
export type ChatFile = { id: string; name: string; mimeType: string; url: string }
export type ChatMessage = {
  id: string
  role: 'user' | 'agent'
  content: string
  createdAt: string
  references?: ChatReference[]
  embeds?: ChatEmbed[]
  files?: ChatFile[]
}
/**
 * `POST /api/client/chat/messages` の `202 Accepted` 応答。応答はバックグラウンドで生成される。
 * `sinceId` より大きい `id` を持つ `role:'agent'` のメッセージが `GET /api/client/chat/messages`
 * に現れたら完了（失敗・タイムアウト時もサーバーが終端のエラー応答を保存するため必ず現れる）。
 */
export type ChatSendAccepted = { status: 'pending'; sinceId: string }
