import { createServer } from 'node:http'
import { deflateSync } from 'node:zlib'
import type { ChatMessage } from '../src/api/contracts'
import { DEFAULT_GEMINI_MODEL, GEMINI_MODELS } from '../src/api/models'

let settings = { googleConnected: true, googleAccount: 'agent@example.com', model: DEFAULT_GEMINI_MODEL as string, persona: '簡潔で、先回りして支援するパーソナルエージェント。' }
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
  // 本番と同じく calendar に生の Google カレンダー ID、calendarName に表示名が入るケース。
  { id: '3', title: 'IV AKIHABARA (仕事)', startsAt: '2026-08-13T23:00:00+09:00', endsAt: '2026-08-14T05:00:00+09:00', calendar: '8fd1586a3fe4629dc96574b5220ceede3128a4ef749b8e5891cb6682d4e7eab0@group.calendar.google.com', calendarName: 'Google カレンダー', color: '#155eef' },
  { id: '4', title: 'Stella出勤', startsAt: '2026-08-20T12:00:00+09:00', endsAt: '2026-08-20T18:00:00+09:00', calendar: '8fd1586a3fe4629dc96574b5220ceede3128a4ef749b8e5891cb6682d4e7eab0@group.calendar.google.com', calendarName: 'Google カレンダー', color: '#155eef' },
]
// 本番と同じ契約（issue #41）: 送信は 202 `{status:'pending', sinceId}` を即返し、応答はバックグラウンドで
// 生成される。クライアントは GET /api/client/chat/messages を sinceId 起点でポーリングする。
// id は本番の `message_logs.id` と同じ「文字列化した連番」（クライアントは数値として比較する）。
let chatSeq = 1
const nextChatId = () => String(++chatSeq)
let chatMessages: ChatMessage[] = [
  { id: '1', role: 'agent', content: 'こんにちは。今日の予定と未完了タスクを確認できます。\n\n- 必要なら、タスクや家計をこのまま追加できます。\n- 詳細は下の参照から開けます。\n- 「グラフ」と送るとグラフ画像とファイル付きの返信、「家計」「予定」でリッチな埋め込み付きの返信、「エラー」で失敗時の終端応答を試せます。', createdAt: '2026-08-13T08:30:00+09:00', references: [{ type: 'calendar', title: '今日の予定', description: 'チーム定例 10:00 — 11:00', href: '/calendar', meta: 'カレンダー' }, { type: 'todo', title: '未完了タスク', description: '2 件のタスクが残っています', href: '/todo', meta: 'タスク' }] },
]
// 応答生成の擬似遅延（ペンディング表示・ポーリングの確認用）。`MOCK_CHAT_DELAY_MS` で変更できる。
const chatDelayMs = Number(process.env.MOCK_CHAT_DELAY_MS ?? 2500)
// 同一ユーザーの同時ターンは 409（本番の InFlightTurns と同じ挙動）。
let turnInFlight = false

// 添付ファイルのモック用に、依存なしで小さな PNG（グラデーションのバナー）を生成する。
const crcTable = Array.from({ length: 256 }, (_, n) => { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; return c >>> 0 })
const crc32 = (buf: Buffer) => { let c = 0xffffffff; for (const byte of buf) c = crcTable[(c ^ byte) & 0xff]! ^ (c >>> 8); return (c ^ 0xffffffff) >>> 0 }
const pngChunk = (type: string, data: Buffer) => { const body = Buffer.concat([Buffer.from(type, 'ascii'), data]); const out = Buffer.alloc(body.length + 8); out.writeUInt32BE(data.length, 0); body.copy(out, 4); out.writeUInt32BE(crc32(body), body.length + 4); return out }
function gradientPng(width: number, height: number) {
  const raw = Buffer.alloc((width * 3 + 1) * height)
  for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) { const o = y * (width * 3 + 1) + 1 + x * 3; raw[o] = Math.round((x / width) * 255); raw[o + 1] = Math.round((y / height) * 200); raw[o + 2] = 200 }
  const header = Buffer.alloc(13); header.writeUInt32BE(width, 0); header.writeUInt32BE(height, 4); header.writeUInt8(8, 8); header.writeUInt8(2, 9)
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), pngChunk('IHDR', header), pngChunk('IDAT', deflateSync(raw)), pngChunk('IEND', Buffer.alloc(0))])
}
const attachments: Record<string, { name: string; mimeType: string; bytes: Buffer }> = {
  '1': { name: 'attachment.png', mimeType: 'image/png', bytes: gradientPng(360, 140) },
  '2': { name: 'attachment.bin', mimeType: 'application/octet-stream', bytes: Buffer.from('mock binary attachment\n') },
}

function buildReply(content: string): Omit<ChatMessage, 'id' | 'createdAt'> {
  if (/エラー/.test(content)) return { role: 'agent', content: '⚠️ 応答の生成中にエラーが発生しました。もう一度お試しください。' }
  if (/グラフ|チャート|ファイル/.test(content)) return {
    role: 'agent', content: 'グラフを作成しました。画像とファイルを添付します。',
    embeds: [{ title: '今月の支出（カテゴリ別）', description: '食費が**最大**で、全体の 45% を占めています。', color: 0x155eef, fields: [{ name: '食費', value: '¥4,280', inline: true }, { name: '交通', value: '¥1,500', inline: true }, { name: '備考', value: '詳細は下のファイルを参照', inline: false }], footer: 'Agent Desk' }],
    files: [{ id: '1', name: 'attachment.png', mimeType: 'image/png', url: '/api/client/chat/attachments/1' }, { id: '2', name: 'attachment.bin', mimeType: 'application/octet-stream', url: '/api/client/chat/attachments/2' }],
  }
  if (/家計|支出|収入|お金/.test(content)) return {
    role: 'agent', content: '家計の状況を確認しました。現在の支出は **¥5,780** です。詳細は参照カードから開けます。',
    embeds: [{ title: '今月の家計', color: 0x12b76a, fields: [{ name: '支出', value: '¥5,780', inline: true }, { name: '収入', value: '¥320,000', inline: true }], footer: '2026-08' }],
    references: [{ type: 'finance', title: '今月の家計', description: '支出 ¥5,780 / 収入 ¥320,000', href: '/finance', meta: '家計' }],
  }
  if (/予定|カレンダー/.test(content)) return {
    role: 'agent', content: '予定を確認しました。次の予定は **チーム定例** です。',
    embeds: [{ title: 'チーム定例', description: '10:00 — 11:00', color: 0xad5f00, footer: '仕事' }],
    references: [{ type: 'calendar', title: '今週の予定', description: '予定をカレンダーで確認', href: '/calendar', meta: 'カレンダー' }],
  }
  return { role: 'agent', content: '承知しました。必要に応じて、関連する記録を下の参照カードから確認できます。', references: [{ type: 'note', title: '共有ノート', description: 'エージェントと共通の前提を編集', href: '/notes', meta: 'ノート' }] }
}
const json = (res: import('node:http').ServerResponse, body: unknown, status = 200, headers: Record<string, string> = {}) => { res.writeHead(status, { 'content-type': 'application/json', 'access-control-allow-origin': '*', ...headers }); res.end(JSON.stringify(body)) }
const read = async (req: import('node:http').IncomingMessage) => { let body = ''; for await (const c of req) body += c; return body ? JSON.parse(body) : {} }

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
  // 共有ログイン画面（frontend/src/overlays/Login.svelte）が起動時にプローブする。
  // 初期セットアップ不要な状態を返し、通常のログインフォームを表示させる。
  if (req.method === 'GET' && url.pathname === '/api/setup/status') return json(res, { needSetup: false })
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
        preset: 'secretary',
        has_token: true,
        is_system_default: true,
        discord_username: 'yuuka-mock',
      }, {
        // PWA のエージェント切り替え欄を出すための 2 つ目の Bot（モックのデータは Bot で分けない）。
        id: 'bot_mock_assistant',
        name: 'アシスタント',
        preset: 'mcp_assistant',
        has_token: false,
        is_system_default: false,
        discord_username: null,
      }],
    })
  }
  // PWA routes are namespaced in production. Keeping the legacy aliases makes
  // this mock useful for the existing administration UI during its migration.
  const apiPath = url.pathname.replace(/^\/api\/(?:pwa|client)(?=\/|$)/, '/api')
  if (req.method === 'GET' && apiPath === '/api/status') return json(res, { status: 'ok', service: 'agent-mock', checkedAt: new Date().toISOString() })
  if (req.method === 'GET' && url.pathname === '/api/settings/google/oauth/url') return json(res, { success: true, url: 'https://example.com/google-authorize' })
  if (apiPath === '/api/settings') {
    if (req.method === 'PUT') {
      // 本番（PUT /api/client/settings）と同じ契約: model は許可リスト外なら 400、persona のみ更新でき、
      // それ以外のキー（旧クライアントが送る maxTokens / temperature、googleConnected 等）は無視する。
      const body = await read(req)
      const requested = typeof body.model === 'string' ? body.model.trim() : ''
      if (requested && !(GEMINI_MODELS as readonly string[]).includes(requested)) return json(res, { message: `model must be one of: ${GEMINI_MODELS.join(', ')}` }, 400)
      settings = { ...settings, model: requested || settings.model, persona: typeof body.persona === 'string' ? body.persona : settings.persona }
    }
    return json(res, settings)
  }
  if (req.method === 'POST' && apiPath === '/api/integrations/google/authorize') return json(res, { authorizationUrl: 'https://example.com/google-authorize' })
  if (apiPath === '/api/shared-note') { if (req.method === 'PUT') note = { ...note, ...await read(req), updatedAt: new Date().toISOString() }; return json(res, note) }
  if (apiPath === '/api/todos') { if (req.method === 'POST') { const todo = { ...await read(req), id: crypto.randomUUID(), completed: false }; todos.unshift(todo); return json(res, todo, 201) }; return json(res, todos) }
  if (req.method === 'PATCH' && apiPath.startsWith('/api/todos/')) { const id = apiPath.split('/').pop(); const todo = todos.find((item) => item.id === id); if (!todo) return json(res, { message: 'Not found' }, 404); Object.assign(todo, await read(req)); return json(res, todo) }
  if (req.method === 'GET' && apiPath === '/api/calendar/events') return json(res, events)
  if (req.method === 'GET' && apiPath === '/api/finance/summary') { const income = transactions.filter(x => x.kind === 'income').reduce((n, x) => n + x.amount, 0); const expense = transactions.filter(x => x.kind === 'expense').reduce((n, x) => n + x.amount, 0); return json(res, { income, expense, balance: income - expense, month: url.searchParams.get('month') }) }
  if (apiPath === '/api/finance/transactions') { if (req.method === 'POST') { const entry = { ...await read(req), id: crypto.randomUUID() }; transactions.unshift(entry); return json(res, entry, 201) }; return json(res, transactions) }
  if (req.method === 'GET' && apiPath.startsWith('/api/chat/attachments/')) {
    const file = attachments[apiPath.split('/').pop() ?? '']
    if (!file) return json(res, { message: 'Not found' }, 404)
    res.writeHead(200, { 'content-type': file.mimeType, 'content-disposition': `${file.mimeType.startsWith('image/') ? 'inline' : 'attachment'}; filename="${file.name}"`, 'access-control-allow-origin': '*' })
    return res.end(file.bytes)
  }
  if (apiPath === '/api/chat/messages') {
    if (req.method === 'POST') {
      const { content } = await read(req)
      if (typeof content !== 'string' || !content.trim()) return json(res, { message: 'content is required' }, 400)
      if (turnInFlight) return json(res, { message: '前のメッセージへの応答がまだ処理中です。応答が届いてから送信してください。' }, 409)
      const sinceId = String(chatSeq)
      chatMessages.push({ id: nextChatId(), role: 'user', content, createdAt: new Date().toISOString() })
      turnInFlight = true
      setTimeout(() => { chatMessages.push({ id: nextChatId(), createdAt: new Date().toISOString(), ...buildReply(content) }); turnInFlight = false }, chatDelayMs)
      return json(res, { status: 'pending', sinceId }, 202)
    }
    return json(res, chatMessages)
  }
  return json(res, { message: 'Not found' }, 404)
}).listen(8787, () => console.log('Mock API listening on http://localhost:8787'))
