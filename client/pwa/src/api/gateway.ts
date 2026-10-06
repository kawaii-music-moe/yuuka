import type { ChatPollOptions } from './chatPolling'
import type { ReceiptImage } from '../utils/receiptImage'
import type { AgentSettings, CalendarEvent, ChatMessage, NewCalendarEvent, FinanceSummary, HealthStatus, SharedNote, Todo, Transaction } from './contracts'

/** UI が依存する唯一の API 契約。バックエンド仕様変更時は adapter だけを変更する。 */
export interface AgentGateway {
  getHealth(): Promise<HealthStatus>
  getSettings(): Promise<AgentSettings>
  saveSettings(settings: AgentSettings): Promise<AgentSettings>
  startGoogleAuthorization(): Promise<{ authorizationUrl: string }>
  getSharedNote(): Promise<SharedNote>
  saveSharedNote(note: Pick<SharedNote, 'title' | 'body'>): Promise<SharedNote>
  listTodos(): Promise<Todo[]>
  createTodo(todo: Pick<Todo, 'title' | 'dueDate' | 'list'>): Promise<Todo>
  updateTodo(id: string, patch: Pick<Todo, 'completed'>): Promise<Todo>
  listCalendarEvents(from: string, to: string): Promise<CalendarEvent[]>
  createCalendarEvent(event: NewCalendarEvent): Promise<CalendarEvent>
  getFinanceSummary(month: string): Promise<FinanceSummary>
  listTransactions(month: string): Promise<Transaction[]>
  createTransaction(transaction: Omit<Transaction, 'id'>): Promise<Transaction>
  /**
   * レシート画像をエージェントに読み取らせ、家計簿へ記帳させる。戻り値はエージェントの報告文。
   * 形式不正（400）・レート制限（429）は `ApiError`（`serverMessage` に案内文）で失敗する。
   */
  scanReceipt(image: ReceiptImage): Promise<{ response: string }>
  listChatMessages(): Promise<ChatMessage[]>
  /**
   * メッセージを送り、エージェントの応答が届くまで待って返す。サーバーは重いターンを非同期実行する
   * （`202` + 履歴のポーリング）ため、この呼び出しは数十秒〜数分かかりうる。`onTick` で待機中の
   * 経過を、`signal` で中断を受け取れる。同期的に拒否された場合（空・キー未設定 400 / 処理中 409 /
   * レート制限 429）は `ApiError`（`serverMessage` に案内文）で即座に失敗する。待ちきれなければ
   * `ChatReplyTimeoutError`。
   */
  sendChatMessage(content: string, options?: Pick<ChatPollOptions, 'onTick' | 'signal'>): Promise<ChatMessage>
  /** 送信済みだが応答待ちのターン（リロード後など）の応答を待つ。`sinceId` は最後に見えた id。 */
  waitForChatReply(sinceId: string, options?: Pick<ChatPollOptions, 'onTick' | 'signal'>): Promise<ChatMessage>
}
