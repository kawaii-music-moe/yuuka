import type { AgentGateway } from './gateway'
import type { AgentSettings, SharedNote, Todo, Transaction } from './contracts'
import { request } from './http'

// このファイルだけが未確定の HTTP req/res 形式を知る。画面・状態管理層は Gateway のみを利用する。
export const httpAgentGateway: AgentGateway = {
  getHealth: () => request('/api/pwa/status'),
  getSettings: () => request('/api/pwa/settings'),
  saveSettings: (settings: AgentSettings) => request('/api/pwa/settings', { method: 'PUT', body: JSON.stringify(settings) }),
  startGoogleAuthorization: async () => {
    const response = await request<{ success: boolean; url: string }>('/api/settings/google/oauth/url')
    return { authorizationUrl: response.url }
  },
  getSharedNote: () => request('/api/pwa/shared-note'),
  saveSharedNote: (note: Pick<SharedNote, 'title' | 'body'>) => request('/api/pwa/shared-note', { method: 'PUT', body: JSON.stringify(note) }),
  listTodos: () => request('/api/pwa/todos'),
  createTodo: (todo: Pick<Todo, 'title' | 'dueDate' | 'list'>) => request('/api/pwa/todos', { method: 'POST', body: JSON.stringify(todo) }),
  updateTodo: (id: string, patch: Pick<Todo, 'completed'>) => request(`/api/pwa/todos/${id}`, { method: 'PATCH', body: JSON.stringify(patch) }),
  listCalendarEvents: (from: string, to: string) => request(`/api/pwa/calendar/events?from=${from}&to=${to}`),
  getFinanceSummary: (month: string) => request(`/api/pwa/finance/summary?month=${month}`),
  listTransactions: (month: string) => request(`/api/pwa/finance/transactions?month=${month}`),
  createTransaction: (transaction: Omit<Transaction, 'id'>) => request('/api/pwa/finance/transactions', { method: 'POST', body: JSON.stringify(transaction) }),
  listChatMessages: () => request('/api/pwa/chat/messages'),
  sendChatMessage: (content: string) => request('/api/pwa/chat/messages', { method: 'POST', body: JSON.stringify({ content }) }),
}
