import { request } from '@/api/http'
import type { ClientBot } from '@/api/contracts'

type ApiBot = ClientBot & { discord_username?: string | null; discord_avatar_url?: string | null; running?: boolean; connected?: boolean; suspended?: boolean | number; preset_display_name?: string }

function toClientBot(bot: ApiBot): ClientBot {
  return { id: bot.id, name: bot.discord_username || bot.name, avatarUrl: bot.discord_avatar_url ?? undefined, preset: bot.preset }
}

export type AdminBot = ReturnType<typeof toClientBot> & Pick<ApiBot, 'running' | 'connected' | 'suspended' | 'preset_display_name'>

export async function listAdminBots(): Promise<AdminBot[]> {
  const response = await request<{ bots?: ApiBot[] }>('/api/bots?scope=user')
  return (response.bots ?? []).map((bot) => ({ ...toClientBot(bot), running: bot.running, connected: bot.connected, suspended: bot.suspended, preset_display_name: bot.preset_display_name }))
}
