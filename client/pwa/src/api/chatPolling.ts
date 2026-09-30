import type { ChatMessage } from './contracts'
import { ApiError } from './http'

/**
 * チャット応答のポーリング（`POST /api/client/chat/messages` が 202 を返した後）。
 *
 * サーバーは重いターンをバックグラウンドで実行するため、応答は `GET /api/client/chat/messages`
 * （履歴）に「`sinceId` より大きい id の `role:'agent'`」として現れる。プロキシのタイムアウトに
 * 依存しないよう短いリクエストを繰り返し、間隔は指数バックオフで伸ばす。
 */
export const CHAT_POLL_INITIAL_DELAY_MS = 800
export const CHAT_POLL_MAX_DELAY_MS = 5000
export const CHAT_POLL_BACKOFF_FACTOR = 1.5
/** サーバー側のターンタイムアウト（180 秒）＋余裕。これを超えたらクライアントは待つのを止める。 */
export const CHAT_POLL_MAX_WAIT_MS = 240_000

/** 応答を待ちきれなかった（サーバーのターンはまだ動いているか、履歴を取得できなかった）。 */
export class ChatReplyTimeoutError extends Error {
  constructor(public readonly waitedMs: number) { super(`No reply within ${waitedMs}ms`) }
}

export type ChatPollOptions = {
  listMessages: () => Promise<ChatMessage[]>
  sinceId: string
  /** 待機（テストで差し替えられるよう注入可能）。AbortSignal で中断できる。 */
  sleep?: (ms: number, signal?: AbortSignal) => Promise<void>
  now?: () => number
  signal?: AbortSignal
  maxWaitMs?: number
  /** 各ポーリングの直前に呼ばれる（経過時間に応じた「時間がかかっています」表示用）。 */
  onTick?: (info: { attempt: number; elapsedMs: number }) => void
}

const defaultSleep = (ms: number, signal?: AbortSignal) => new Promise<void>((resolve, reject) => {
  if (signal?.aborted) return reject(signal.reason ?? new DOMException('Aborted', 'AbortError'))
  const timer = setTimeout(() => { signal?.removeEventListener('abort', onAbort); resolve() }, ms)
  const onAbort = () => { clearTimeout(timer); reject(signal?.reason ?? new DOMException('Aborted', 'AbortError')) }
  signal?.addEventListener('abort', onAbort, { once: true })
})

/** id は文字列化された整数（`message_logs.id`）。数値として比較する。 */
export function isReplyAfter(message: ChatMessage, sinceId: string): boolean {
  if (message.role !== 'agent') return false
  const id = Number(message.id)
  const since = Number(sinceId)
  return Number.isFinite(id) && Number.isFinite(since) && id > since
}

export async function pollForChatReply(options: ChatPollOptions): Promise<ChatMessage> {
  const { listMessages, sinceId, signal, onTick, maxWaitMs = CHAT_POLL_MAX_WAIT_MS } = options
  const sleep = options.sleep ?? defaultSleep
  const now = options.now ?? Date.now
  const startedAt = now()
  let delay = CHAT_POLL_INITIAL_DELAY_MS
  for (let attempt = 1; ; attempt++) {
    const elapsedMs = now() - startedAt
    if (elapsedMs >= maxWaitMs) throw new ChatReplyTimeoutError(elapsedMs)
    onTick?.({ attempt, elapsedMs })
    await sleep(Math.min(delay, Math.max(0, maxWaitMs - elapsedMs)), signal)
    try {
      const reply = (await listMessages()).find((message) => isReplyAfter(message, sinceId))
      if (reply) return reply
    } catch (error) {
      // 一時的な通信失敗（電波が途切れた等）では諦めず、最大待機時間まで続ける。中断・認証切れ
      // （再ログインが必要）は待っても回復しないためそのまま投げる。
      if (signal?.aborted || (error instanceof ApiError && (error.status === 401 || error.status === 403))) throw error
    }
    delay = Math.min(delay * CHAT_POLL_BACKOFF_FACTOR, CHAT_POLL_MAX_DELAY_MS)
  }
}
