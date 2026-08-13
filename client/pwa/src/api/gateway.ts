import type { AgentSettings, CalendarEvent, ChatMessage, FinanceSummary, HealthStatus, SharedNote, Todo, Transaction } from './contracts'

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
  getFinanceSummary(month: string): Promise<FinanceSummary>
  listTransactions(month: string): Promise<Transaction[]>
  createTransaction(transaction: Omit<Transaction, 'id'>): Promise<Transaction>
  listChatMessages(): Promise<ChatMessage[]>
  sendChatMessage(content: string): Promise<ChatMessage>
}
