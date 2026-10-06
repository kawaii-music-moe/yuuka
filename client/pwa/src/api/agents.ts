import { ref } from 'vue'
import { request } from './http'

/** 切り替え先のエージェント（サーバーの Bot）。 */
export type Agent = { id: string; name: string }

/** 秘書 Bot。`botId` を付けないリクエストはサーバーでもこれになる。 */
export const DEFAULT_AGENT_ID = 'system_default'

const STORAGE_KEY = 'yuuka.pwa.agentId'

function loadStoredAgentId(): string {
  try {
    return window.localStorage.getItem(STORAGE_KEY) || DEFAULT_AGENT_ID
  } catch {
    // プライベートブラウズ等で storage が使えなくても既定のエージェントで動かす。
    return DEFAULT_AGENT_ID
  }
}

/** 選択中のエージェント。`/api/client/*` はすべてこの Bot のデータを読み書きする。 */
export const selectedAgentId = ref(loadStoredAgentId())

export function selectAgent(id: string) {
  selectedAgentId.value = id
  try {
    if (id === DEFAULT_AGENT_ID) window.localStorage.removeItem(STORAGE_KEY)
    else window.localStorage.setItem(STORAGE_KEY, id)
  } catch {
    // 保存できなくても、このタブの中では切り替わったまま動く。
  }
}

/** `/api/client/*` のパスに選択中エージェントの `botId` を付ける（秘書 Bot なら付けない）。 */
export function withSelectedAgent(path: string): string {
  const agentId = selectedAgentId.value
  if (agentId === DEFAULT_AGENT_ID) return path
  return `${path}${path.includes('?') ? '&' : '?'}botId=${encodeURIComponent(agentId)}`
}

type BotsResponse = { bots: { id: string; name: string }[] }

/**
 * 切り替えられるエージェント（秘書 Bot・自分の Bot・共有された Bot）を返す。保存されていた選択が
 * 一覧に無い（削除・共有解除された）場合は秘書 Bot に戻す（サーバーはその botId を 404 にする）。
 */
export async function listAgents(): Promise<Agent[]> {
  const { bots } = await request<BotsResponse>('/api/bots')
  const agents = bots.map((bot) => ({ id: bot.id, name: bot.name }))
  if (!agents.some((agent) => agent.id === selectedAgentId.value)) selectAgent(DEFAULT_AGENT_ID)
  return agents
}
