import { selectedBot } from '@/stores/botSelection'

export class ApiError extends Error {
  constructor(message: string, public readonly status?: number) { super(message) }
}

export async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const url = new URL(path, window.location.origin)
  if (selectedBot.value?.id) url.searchParams.set('bot_id', selectedBot.value.id)
  const response = await fetch(`${url.pathname}${url.search}`, {
    ...init,
    // `include` keeps the session cookie working both through Vite's dev
    // proxy and when the Client is served from the Yuuka origin.
    credentials: 'include',
    headers: { 'content-type': 'application/json', ...init?.headers },
  })
  if (!response.ok) throw new ApiError(`Request failed: ${response.status}`, response.status)
  return response.json() as Promise<T>
}
