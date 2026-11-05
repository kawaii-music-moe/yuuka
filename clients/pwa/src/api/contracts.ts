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
/** チャット送信の添付（base64）。 */
export type ChatAttachment = { name: string; mimeType: string; dataBase64: string }
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
/**
 * 音声チャットの声（`GET /api/client/voices`・Irodori-TTS の話者条件）。ユーザー本人のもので、エージェントには
 * 依らない。話者条件の本体（f32 の tokens × 768）は `GET /api/client/voices/:id/state` で取得する。
 */
export type Voice = {
  id: string
  name: string
  /** 話し方の指定（キャプション）。空なら指定なし。 */
  caption: string
  /** `sample` は声のサンプルから、`design` はキャプションと声の番号から作った声。 */
  source: 'sample' | 'design'
  /** 元にした音声の長さ（秒）。 */
  seconds: number
  /** 話者条件を作ったモデルの系統。今のモデルと違えば使えない。 */
  speakerModel: string
  tokens: number
  mask: boolean[]
  createdAt: string
  updatedAt: string
}
/** `POST /api/client/voices`。`stateBase64` は話者条件（f32 リトルエンディアン）の base64。 */
export type NewVoice = Pick<Voice, 'name' | 'caption' | 'source' | 'seconds' | 'speakerModel' | 'mask'> & { stateBase64: string }
