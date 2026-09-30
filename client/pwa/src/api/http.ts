export class ApiError extends Error {
  /**
   * @param serverMessage サーバーが返した `{ message }`（400/409/429 など、ユーザーに見せて良い
   *   日本語の案内文）。JSON でない / 無い場合は undefined。
   */
  constructor(message: string, public readonly status?: number, public readonly serverMessage?: string) { super(message) }
}

/** 失敗応答の `{ message }` を取り出す（JSON でなければ undefined）。 */
async function readServerMessage(response: Response): Promise<string | undefined> {
  try {
    const body: unknown = await response.json()
    if (body && typeof body === 'object' && 'message' in body && typeof body.message === 'string') return body.message
  } catch { /* 本文なし / JSON でない */ }
  return undefined
}

export async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, {
    ...init,
    // `include` keeps the session cookie working both through Vite's dev
    // proxy and when the Client is served from the Yuuka origin.
    credentials: 'include',
    headers: { 'content-type': 'application/json', ...init?.headers },
  })
  if (!response.ok) throw new ApiError(`Request failed: ${response.status}`, response.status, await readServerMessage(response))
  return response.json() as Promise<T>
}
