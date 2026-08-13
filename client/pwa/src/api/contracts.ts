export type HealthStatus = { status: 'ok' | 'error'; service: string; checkedAt: string }
export type AgentSettings = {
  googleConnected: boolean
  googleAccount?: string
  model: string
  maxTokens: number
  temperature: number
  persona: string
}
export type SharedNote = { id: string; title: string; body: string; updatedAt: string }
export type Todo = { id: string; title: string; dueDate?: string; completed: boolean; list: string }
export type CalendarEvent = { id: string; title: string; startsAt: string; endsAt: string; calendar: string; color: string }
export type Transaction = { id: string; date: string; category: string; description: string; amount: number; kind: 'income' | 'expense' }
export type FinanceSummary = { income: number; expense: number; balance: number; month: string }
export type ChatReference = {
  type: 'todo' | 'calendar' | 'finance' | 'note'
  id?: string
  title: string
  description: string
  href: string
  meta?: string
}
export type ChatMessage = {
  id: string
  role: 'user' | 'agent'
  content: string
  createdAt: string
  references?: ChatReference[]
}
