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
const json = (res: import('node:http').ServerResponse, body: unknown, status = 200, headers: Record<string, string> = {}) => { res.writeHead(status, { 'content-type': 'application/json', 'access-control-allow-origin': '*', ...headers }); res.end(JSON.stringify(body)) }
const read = async (req: import('node:http').IncomingMessage) => { let body = ''; for await (const c of req) body += c; return body ? JSON.parse(body) : {} }

/**
 * 管理画面の各タブが初期表示時に使う最小データ。
 * 実APIの形式が確定しても、このモックだけを拡張すればフロント本体は変更不要。
 */
function managementFixture(path: string): Record<string, unknown> {
  if (/^\/api\/integrated\/google\/accounts\/\d+\/calendars$/.test(path)) {
    return { success: true, calendars: [{ id: 'primary', summary: 'メインカレンダー', selected: true }] }
  }
  if (/^\/api\/mcp-servers\/\d+\/dashboard\/status$/.test(path)) return { success: true, available: false }
  if (/^\/api\/timeline\/media\//.test(path)) return { success: true }

  const fixtures: Record<string, Record<string, unknown>> = {
    '/api/tasks': { success: true, tasks: [] },
    '/api/tasks/gantt': { success: true, tasks: [] },
    '/api/tasks/someday': { success: true, tasks: [] },
    '/api/tasks/detail': { success: true, task: null, logs: [] },
    '/api/schedules': { success: true, schedules: [] },
    '/api/expenses': { success: true, expenses: [], total: 0, incomeTotal: 0, breakdown: [], trend: [] },
    '/api/expenses/budget-limits': { success: true, limits: [] },
    '/api/expenses/plans': { success: true, plans: [] },
    '/api/reminders': { success: true, reminders: [] },
    '/api/context-note': { success: true, content: '', max_length: 4000 },
    '/api/clipboard': { success: true, entries: [] },
    '/api/contacts': { success: true, contacts: [] },
    '/api/personas': { success: true, personas: [], active_persona_id: null, max_length: 4000 },
    '/api/briefing-config': { success: true, config: null },
    '/api/report-configs': { success: true, configs: [] },
    '/api/webhooks': { success: true, endpoints: [] },
    '/api/webhooks/deliveries': { success: true, deliveries: [] },
    '/api/mcp-servers': { success: true, servers: [] },
    '/api/playbooks': { success: true, playbooks: [] },
    '/api/playbooks/schedules': { success: true, schedules: [] },
    '/api/playbooks/runs': { success: true, runs: [] },
    '/api/timeline/day': { success: true, plans: [], records: [] },
    '/api/devices': { success: true, devices: [] },
    '/api/desktop/info': { success: true, available: false },
    '/api/credentials': { success: true, credentials: [] },
    '/api/integrated/bots/mcp': { success: true, bot_id: 'system_default', is_owner: true, servers: [], own_servers: [] },
    '/api/integrated/overview': {
      success: true,
      bots: [{ id: 'system_default', name: '既定の秘書', preset: 'mcp_assistant', running: true, connected: true, is_system_default: true, granted_credentials: [], granted_mcp_ids: [] }],
      credentials: [], mcpServers: [], googleAccounts: [],
    },
    '/api/settings/discord': { success: true, configured: false, tokenSet: false },
    '/api/bots/presets': { success: true, presets: [{ preset: 'mcp_assistant', display_name: '汎用モード' }, { preset: 'secretary', display_name: 'パーソナル秘書' }] },
    '/api/bots/modules': { success: true, modules: [] },
    '/api/bots/assistant-config': { success: true, config: {} },
    '/api/bots/assistant/guild-options': { success: true, guilds: [] },
    '/api/bots/assistant/guild-note': { success: true, content: '' },
    '/api/bots/shares': { success: true, shares: [] },
    '/api/bots/member-requests': { success: true, requests: [] },
  }
  return fixtures[path] ?? { success: true }
}

createServer(async (req, res) => {
  if (req.method === 'OPTIONS') { res.writeHead(204, { 'access-control-allow-origin': '*', 'access-control-allow-methods': 'GET,POST,PUT,PATCH,OPTIONS', 'access-control-allow-headers': 'content-type' }); return res.end() }
  const url = new URL(req.url ?? '/', 'http://localhost:8787')
  const hasSession = req.headers.cookie?.includes('yuuka-mock-session=admin') ?? false
  if (req.method === 'POST' && url.pathname === '/api/login') {
    const { discordId, password } = await read(req)
    if (discordId !== 'admin' || password !== 'pass') return json(res, { message: 'Invalid credentials' }, 401)
    return json(res, { success: true }, 200, { 'Set-Cookie': 'yuuka-mock-session=admin; Path=/; SameSite=Lax' })
  }
  if (req.method === 'POST' && url.pathname === '/api/logout') return json(res, { success: true }, 200, { 'Set-Cookie': 'yuuka-mock-session=; Path=/; Max-Age=0' })
  if (req.method === 'GET' && url.pathname === '/api/me') {
    if (!hasSession) return json(res, { message: 'Unauthorized' }, 401)
    // Keep this response compatible with the production authentication API.
    // The administration UI relies on `success`, while the Client consumes `user`.
    return json(res, { success: true, user: { discordId: 'admin', username: 'admin', role: 'admin' } })
  }
  // The administration application verifies that the system bot already has a
  // token immediately after an admin signs in. The mock represents a ready-to-
  // use development environment, so it must report that prerequisite as met.
  if (req.method === 'GET' && url.pathname === '/api/bots') {
    if (!hasSession) return json(res, { success: false, message: 'Unauthorized' }, 401)
    return json(res, {
      success: true,
      bots: [{
        id: 'system_default',
        name: '既定の秘書',
        preset: 'mcp_assistant',
        has_token: true,
        is_system_default: true,
        discord_username: 'yuuka-mock',
      }, {
        id: 'bot_mock_planner',
        name: 'Planner mock',
        preset: 'secretary',
        has_token: true,
        running: true,
        connected: true,
        discord_username: 'planner-mock',
      }],
    })
  }
  if (req.method === 'GET' && url.pathname === '/api/bots/usage') {
    if (!hasSession) return json(res, { success: false, message: 'Unauthorized' }, 401)
    return json(res, {
      success: true,
      days: 7,
      series: [
        { date: '2026-08-08', requests: 18, responses: 17 },
        { date: '2026-08-09', requests: 24, responses: 24 },
        { date: '2026-08-10', requests: 12, responses: 12 },
        { date: '2026-08-11', requests: 31, responses: 30 },
        { date: '2026-08-12', requests: 27, responses: 26 },
        { date: '2026-08-13', requests: 39, responses: 38 },
        { date: '2026-08-14', requests: 22, responses: 21 },
      ],
      totals: { requests: 173, responses: 168 },
      rate_limits: { userPerMinute: 5, userPerDay: 100, guildPerDay: 1000 },
    })
  }
  // Administration dashboard bootstrap. These endpoints are requested in
  // parallel when /admin opens, so returning the production-shaped empty
  // state prevents a burst of misleading "Not found" notifications in mock
  // development.
  if (req.method === 'GET' && url.pathname === '/api/admin/stats') {
    return json(res, { success: true, stats: { users: 1, bots: 1, suspendedBots: 0, inviteCodes: 0 } })
  }
  if (req.method === 'GET' && url.pathname === '/api/admin/users') {
    return json(res, { success: true, users: [{ discord_id: 'admin', username: 'admin', role: 'admin' }] })
  }
  if (req.method === 'GET' && url.pathname === '/api/admin/bots') {
    return json(res, { success: true, bots: [{ id: 'system_default', name: '既定の秘書', preset: 'secretary', has_token: true, running: true, suspended: false, is_system_default: true }] })
  }
  if (req.method === 'GET' && url.pathname === '/api/admin/invite-codes') return json(res, { success: true, codes: [] })
  if (req.method === 'GET' && url.pathname === '/api/admin/audit-logs') return json(res, { success: true, logs: [], total: 0 })
  if (req.method === 'GET' && url.pathname === '/api/admin/system-settings') return json(res, { success: true, privacyPolicyUrl: '', termsUrl: '' })
  if (req.method === 'GET' && url.pathname === '/api/admin/bot-attribute-settings') {
    return json(res, { success: true, presets: [{ id: 'secretary', displayName: 'パーソナル秘書' }, { id: 'mcp_assistant', displayName: '汎用モード' }], rate_limits: { userPerMinute: 5, userPerDay: 100, guildPerDay: 1000 } })
  }
  if (req.method === 'GET' && url.pathname === '/api/personas/marketplace') return json(res, { success: true, personas: [] })
  // PWA routes are namespaced in production. Keeping the legacy aliases makes
  // this mock useful for the existing administration UI during its migration.
  const apiPath = url.pathname.replace(/^\/api\/(?:pwa|client)(?=\/|$)/, '/api')
  if (req.method === 'GET' && apiPath === '/api/status') return json(res, { status: 'ok', service: 'agent-mock', checkedAt: new Date().toISOString() })
  if (req.method === 'GET' && url.pathname === '/api/settings/google/oauth/url') return json(res, { success: true, url: 'https://example.com/google-authorize' })
  if (apiPath === '/api/settings') { if (req.method === 'PUT') settings = await read(req); return json(res, settings) }
  if (req.method === 'POST' && apiPath === '/api/integrations/google/authorize') return json(res, { authorizationUrl: 'https://example.com/google-authorize' })
  if (apiPath === '/api/shared-note') { if (req.method === 'PUT') note = { ...note, ...await read(req), updatedAt: new Date().toISOString() }; return json(res, note) }
  if (apiPath === '/api/todos') { if (req.method === 'POST') { const todo = { ...await read(req), id: crypto.randomUUID(), completed: false }; todos.unshift(todo); return json(res, todo, 201) }; return json(res, todos) }
  if (req.method === 'PATCH' && apiPath.startsWith('/api/todos/')) { const id = apiPath.split('/').pop(); const todo = todos.find((item) => item.id === id); if (!todo) return json(res, { message: 'Not found' }, 404); Object.assign(todo, await read(req)); return json(res, todo) }
  if (req.method === 'GET' && apiPath === '/api/calendar/events') return json(res, events)
  if (req.method === 'GET' && apiPath === '/api/finance/summary') { const income = transactions.filter(x => x.kind === 'income').reduce((n, x) => n + x.amount, 0); const expense = transactions.filter(x => x.kind === 'expense').reduce((n, x) => n + x.amount, 0); return json(res, { income, expense, balance: income - expense, month: url.searchParams.get('month') }) }
  if (apiPath === '/api/finance/transactions') { if (req.method === 'POST') { const entry = { ...await read(req), id: crypto.randomUUID() }; transactions.unshift(entry); return json(res, entry, 201) }; return json(res, transactions) }
  if (apiPath === '/api/chat/messages') {
    if (req.method === 'POST') {
      const { content } = await read(req); const now = new Date().toISOString()
      chatMessages.push({ id: crypto.randomUUID(), role: 'user', content, createdAt: now })
      const hasFinance = /家計|支出|収入|お金/.test(content); const hasCalendar = /予定|カレンダー/.test(content)
      const reply: ChatMessage = { id: crypto.randomUUID(), role: 'agent', createdAt: new Date().toISOString(), content: hasFinance ? '家計の状況を確認しました。現在の支出は **¥5,780** です。詳細は参照カードから開けます。' : hasCalendar ? '予定を確認しました。次の予定は **チーム定例** です。' : '承知しました。必要に応じて、関連する記録を下の参照カードから確認できます。', references: hasFinance ? [{ type: 'finance', title: '今月の家計', description: '支出 ¥5,780 / 収入 ¥320,000', href: '/finance', meta: '家計' }] : hasCalendar ? [{ type: 'calendar', title: '今週の予定', description: '予定をカレンダーで確認', href: '/calendar', meta: 'カレンダー' }] : [{ type: 'note', title: '共有ノート', description: 'エージェントと共通の前提を編集', href: '/notes', meta: 'ノート' }] }
      chatMessages.push(reply); return json(res, reply, 201)
    }
    return json(res, chatMessages)
  }
  // 管理画面の開発中は、未追加の操作APIも成功として扱う。
  // 新しい画面を先に実装しても404トーストで操作確認が妨げられず、必要になった時点で
  // managementFixture に実データの形を足していける。
  if (url.pathname.startsWith('/api/')) {
    if (!hasSession) return json(res, { success: false, message: 'Unauthorized' }, 401)
    if (req.method === 'GET') return json(res, managementFixture(url.pathname))
    return json(res, { success: true })
  }
  return json(res, { message: 'Not found' }, 404)
}).listen(8787, () => console.log('Mock API listening on http://localhost:8787'))
