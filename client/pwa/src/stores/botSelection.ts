import { ref } from 'vue'
import type { ClientBot } from '@/api/contracts'

const storageKey = 'yuuka.client.selected-bot'

function load(): ClientBot | null {
  try {
    const raw = localStorage.getItem(storageKey)
    return raw ? JSON.parse(raw) as ClientBot : null
  } catch {
    return null
  }
}

export const availableBots = ref<ClientBot[]>([])
export const selectedBot = ref<ClientBot | null>(load())

export function setAvailableBots(bots: ClientBot[]): void {
  availableBots.value = bots
}

export function selectClientBot(bot: ClientBot): void {
  selectedBot.value = bot
  localStorage.setItem(storageKey, JSON.stringify(bot))
}

export function selectClientBotById(botId: string | undefined): void {
  if (!botId) return
  const bot = availableBots.value.find((item) => item.id === botId)
  if (bot) selectClientBot(bot)
}
