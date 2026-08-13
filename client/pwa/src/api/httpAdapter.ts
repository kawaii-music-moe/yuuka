import type { AgentGateway } from './gateway'
import type { AgentSettings, ClientBot, SharedNote, Todo, Transaction } from './contracts'
import { request } from './http'

// このファイルだけが未確定の HTTP req/res 形式を知る。画面・状態管理層は Gateway のみを利用する。
export const httpAgentGateway: AgentGateway = {
  listBots: async () => {
    const response = await request<{ bots?: ClientBot[] }>('/api/bots?scope=user')
    return response.bots ?? []
  },
  getHealth: () => request('/api/client/status'),
  getSettings: () => request('/api/client/settings'),
  saveSettings: (settings: AgentSettings) => request('/api/client/settings', { method: 'PUT', body: JSON.stringify(settings) }),
  startGoogleAuthorization: async () => {
    const response = await request<{ success: boolean; url: string }>('/api/settings/google/oauth/url')
    return { authorizationUrl: response.url }
  },
  getSharedNote: () => request('/api/client/shared-note'),
  saveSharedNote: (note: Pick<SharedNote, 'title' | 'body'>) => request('/api/client/shared-note', { method: 'PUT', body: JSON.stringify(note) }),
  listTodos: () => request('/api/client/todos'),
  createTodo: (todo: Pick<Todo, 'title' | 'dueDate' | 'list'>) => request('/api/client/todos', { method: 'POST', body: JSON.stringify(todo) }),
  updateTodo: (id: string, patch: Pick<Todo, 'completed'>) => request(`/api/client/todos/${id}`, { method: 'PATCH', body: JSON.stringify(patch) }),
  listCalendarEvents: (from: string, to: string) => request(`/api/client/calendar/events?from=${from}&to=${to}`),
  getFinanceSummary: (month: string) => request(`/api/client/finance/summary?month=${month}`),
  listTransactions: (month: string) => request(`/api/client/finance/transactions?month=${month}`),
  createTransaction: (transaction: Omit<Transaction, 'id'>) => request('/api/client/finance/transactions', { method: 'POST', body: JSON.stringify(transaction) }),
  listChatMessages: () => request('/api/client/chat/messages'),
  sendChatMessage: (content: string) => request('/api/client/chat/messages', { method: 'POST', body: JSON.stringify({ content }) }),
}
