import { createServer } from 'node:http'
import type { ChatMessage } from '../src/api/contracts'

let settings = { googleConnected: true, googleAccount: 'agent@example.com', model: 'GPT-4o', maxTokens: 2048, temperature: 0.7, persona: '簡潔で、先回りして支援するパーソナルエージェント。' }
let note = { id: 'shared-note', title: '共有ノート', body: '# 今週の方針\n\n- 平日の予定は朝に確認する\n- 支出は当日中に記録する', updatedAt: new Date().toISOString() }
let todos = [
  { id: '1', title: '今週の買い物リストを整理', dueDate: '2026-08-14', completed: false, list: '個人' },
  { id: '2', title: '定例資料を確認', dueDate: '2026-08-13', completed: false, list: '仕事' },
  { id: '3', title: '電気料金を支払う', completed: true, list: '個人' },
]
let transactions = [
  { id: '1', date: '2026-08-12', category: '食費', description: 'スーパーマーケット', amount: 4280, kind: 'expense' },
  { id: '2', date: '2026-08-10', category: '交通', description: '交通系IC', amount: 1500, kind: 'expense' },
  { id: '3', date: '2026-08-01', category: '給与', description: '給与', amount: 320000, kind: 'income' },
]
const events = [
  { id: '1', title: 'チーム定例', startsAt: '2026-08-13T10:00:00+09:00', endsAt: '2026-08-13T11:00:00+09:00', calendar: '仕事', color: '#126a57' },
  { id: '2', title: '歯科検診', startsAt: '2026-08-15T15:30:00+09:00', endsAt: '2026-08-15T16:30:00+09:00', calendar: '個人', color: '#ad5f00' },
]
let chatMessages: ChatMessage[] = [
  { id: 'chat-1', role: 'agent', content: 'こんにちは。今日の予定と未完了タスクを確認できます。\n\n- 必要なら、タスクや家計をこのまま追加できます。\n- 詳細は下の参照から開けます。', createdAt: '2026-08-13T08:30:00+09:00', references: [{ type: 'calendar', title: '今日の予定', description: 'チーム定例 10:00 — 11:00', href: '/calendar', meta: 'カレンダー' }, { type: 'todo', title: '未完了タスク', description: '2 件のタスクが残っています', href: '/todo', meta: 'タスク' }] },
]
const json = (res: import('node:http').ServerResponse, body: unknown, status = 200) => { res.writeHead(status, { 'content-type': 'application/json', 'access-control-allow-origin': '*' }); res.end(JSON.stringify(body)) }
const read = async (req: import('node:http').IncomingMessage) => { let body = ''; for await (const c of req) body += c; return body ? JSON.parse(body) : {} }

createServer(async (req, res) => {
  if (req.method === 'OPTIONS') { res.writeHead(204, { 'access-control-allow-origin': '*', 'access-control-allow-methods': 'GET,POST,PUT,PATCH,OPTIONS', 'access-control-allow-headers': 'content-type' }); return res.end() }
  const url = new URL(req.url ?? '/', 'http://localhost:8787')
  if (req.method === 'GET' && url.pathname === '/api/status') return json(res, { status: 'ok', service: 'agent-mock', checkedAt: new Date().toISOString() })
  if (url.pathname === '/api/settings') { if (req.method === 'PUT') settings = await read(req); return json(res, settings) }
  if (req.method === 'POST' && url.pathname === '/api/integrations/google/authorize') return json(res, { authorizationUrl: 'https://example.com/google-authorize' })
  if (url.pathname === '/api/shared-note') { if (req.method === 'PUT') note = { ...note, ...await read(req), updatedAt: new Date().toISOString() }; return json(res, note) }
  if (url.pathname === '/api/todos') { if (req.method === 'POST') { const todo = { ...await read(req), id: crypto.randomUUID(), completed: false }; todos.unshift(todo); return json(res, todo, 201) }; return json(res, todos) }
  if (req.method === 'PATCH' && url.pathname.startsWith('/api/todos/')) { const id = url.pathname.split('/').pop(); const todo = todos.find((item) => item.id === id); if (!todo) return json(res, { message: 'Not found' }, 404); Object.assign(todo, await read(req)); return json(res, todo) }
  if (req.method === 'GET' && url.pathname === '/api/calendar/events') return json(res, events)
  if (req.method === 'GET' && url.pathname === '/api/finance/summary') { const income = transactions.filter(x => x.kind === 'income').reduce((n, x) => n + x.amount, 0); const expense = transactions.filter(x => x.kind === 'expense').reduce((n, x) => n + x.amount, 0); return json(res, { income, expense, balance: income - expense, month: url.searchParams.get('month') }) }
  if (url.pathname === '/api/finance/transactions') { if (req.method === 'POST') { const entry = { ...await read(req), id: crypto.randomUUID() }; transactions.unshift(entry); return json(res, entry, 201) }; return json(res, transactions) }
  if (url.pathname === '/api/chat/messages') {
    if (req.method === 'POST') {
      const { content } = await read(req); const now = new Date().toISOString()
      chatMessages.push({ id: crypto.randomUUID(), role: 'user', content, createdAt: now })
      const hasFinance = /家計|支出|収入|お金/.test(content); const hasCalendar = /予定|カレンダー/.test(content)
      const reply: ChatMessage = { id: crypto.randomUUID(), role: 'agent', createdAt: new Date().toISOString(), content: hasFinance ? '家計の状況を確認しました。現在の支出は **¥5,780** です。詳細は参照カードから開けます。' : hasCalendar ? '予定を確認しました。次の予定は **チーム定例** です。' : '承知しました。必要に応じて、関連する記録を下の参照カードから確認できます。', references: hasFinance ? [{ type: 'finance', title: '今月の家計', description: '支出 ¥5,780 / 収入 ¥320,000', href: '/finance', meta: '家計' }] : hasCalendar ? [{ type: 'calendar', title: '今週の予定', description: '予定をカレンダーで確認', href: '/calendar', meta: 'カレンダー' }] : [{ type: 'note', title: '共有ノート', description: 'エージェントと共通の前提を編集', href: '/notes', meta: 'ノート' }] }
      chatMessages.push(reply); return json(res, reply, 201)
    }
    return json(res, chatMessages)
  }
  return json(res, { message: 'Not found' }, 404)
}).listen(8787, () => console.log('Mock API listening on http://localhost:8787'))
