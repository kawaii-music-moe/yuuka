import { ApiError, request } from './http'

export type SessionUser = { discordId: string; username: string; role: 'user' | 'admin' }

export async function getCurrentUser(): Promise<SessionUser | null> {
  try {
    const response = await request<{ user: SessionUser }>('/api/me')
    return response.user
  } catch (error) {
    if (error instanceof ApiError && error.status === 401) return null
    throw error
  }
}

export async function login(discordId: string, password: string): Promise<void> {
  await request('/api/login', { method: 'POST', body: JSON.stringify({ discordId, password }) })
}

export async function logout(): Promise<void> {
  await request('/api/logout', { method: 'POST' })
}
