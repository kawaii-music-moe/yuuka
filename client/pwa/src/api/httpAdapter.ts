import type { AgentGateway } from './gateway'
import type { AgentSettings, ChatAttachment, ChatMessage, ChatSendAccepted, NewCalendarEvent, SharedNote, Todo, Transaction } from './contracts'
import { pollForChatReply, type ChatPollOptions } from './chatPolling'
import { withSelectedAgent } from './agents'
import { request } from './http'
import type { ReceiptImage } from '../utils/receiptImage'

// `/api/client/*` は選択中エージェント（Bot）のデータを読み書きする（`?botId=`・`agents.ts`）。
const clientRequest = <T>(path: string, init?: RequestInit) => request<T>(withSelectedAgent(path), init)

const listChatMessages = () => clientRequest<ChatMessage[]>('/api/client/chat/messages')

// このファイルだけが未確定の HTTP req/res 形式を知る。画面・状態管理層は Gateway のみを利用する。
export const httpAgentGateway: AgentGateway = {
  getHealth: () => clientRequest('/api/client/status'),
  getSettings: () => clientRequest('/api/client/settings'),
  saveSettings: (settings: AgentSettings) => clientRequest('/api/client/settings', { method: 'PUT', body: JSON.stringify(settings) }),
  startGoogleAuthorization: async () => {
    const response = await request<{ success: boolean; url: string }>('/api/settings/google/oauth/url')
    return { authorizationUrl: response.url }
  },
  getSharedNote: () => clientRequest('/api/client/shared-note'),
  saveSharedNote: (note: Pick<SharedNote, 'title' | 'body'>) => clientRequest('/api/client/shared-note', { method: 'PUT', body: JSON.stringify(note) }),
  listTodos: () => clientRequest('/api/client/todos'),
  createTodo: (todo: Pick<Todo, 'title' | 'dueDate' | 'list'>) => clientRequest('/api/client/todos', { method: 'POST', body: JSON.stringify(todo) }),
  updateTodo: (id: string, patch: Pick<Todo, 'completed'>) => clientRequest(`/api/client/todos/${id}`, { method: 'PATCH', body: JSON.stringify(patch) }),
  listCalendarEvents: (from: string, to: string) => clientRequest(`/api/client/calendar/events?from=${from}&to=${to}`),
  createCalendarEvent: (event: NewCalendarEvent) => clientRequest('/api/client/calendar/events', { method: 'POST', body: JSON.stringify(event) }),
  getFinanceSummary: (month: string) => clientRequest(`/api/client/finance/summary?month=${month}`),
  listTransactions: (month: string) => clientRequest(`/api/client/finance/transactions?month=${month}`),
  createTransaction: (transaction: Omit<Transaction, 'id'>) => clientRequest('/api/client/finance/transactions', { method: 'POST', body: JSON.stringify(transaction) }),
  scanReceipt: (image: ReceiptImage) => clientRequest('/api/client/finance/receipt', { method: 'POST', body: JSON.stringify(image) }),
  listChatMessages,
  // 202 Accepted（応答はバックグラウンド生成）→ 履歴を sinceId 起点でポーリングして完了を検知する。
  sendChatMessage: async (content: string, attachments: ChatAttachment[], options?: Pick<ChatPollOptions, 'onTick' | 'signal'>) => {
    const accepted = await clientRequest<ChatSendAccepted>('/api/client/chat/messages', { method: 'POST', body: JSON.stringify({ content, attachments }), signal: options?.signal })
    return pollForChatReply({ listMessages: listChatMessages, sinceId: accepted.sinceId, ...options })
  },
  waitForChatReply: (sinceId: string, options?: Pick<ChatPollOptions, 'onTick' | 'signal'>) => pollForChatReply({ listMessages: listChatMessages, sinceId, ...options }),
}
