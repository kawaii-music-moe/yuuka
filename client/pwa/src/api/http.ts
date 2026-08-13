export class ApiError extends Error {
  constructor(message: string, public readonly status?: number) { super(message) }
}

export async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, {
    ...init,
    // `include` keeps the session cookie working both through Vite's dev
    // proxy and when the PWA is served by Yuuka at /pwa/.
    credentials: 'include',
    headers: { 'content-type': 'application/json', ...init?.headers },
  })
  if (!response.ok) throw new ApiError(`Request failed: ${response.status}`, response.status)
  return response.json() as Promise<T>
}
