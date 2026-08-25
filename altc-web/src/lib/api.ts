/**
 * The agent's API. Types here are hand-written for now; ts-rs generation from
 * the Rust structs comes next, so the client cannot compile against a shape
 * the server does not serve.
 */
export interface ServerInfo {
  name: string
  version: string
  setup_complete: boolean
}

export interface User {
  id: number
  name: string
  role: 'owner' | 'admin' | 'member'
  locale: string | null
}

export class ApiError extends Error {}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    ...init,
    headers: { 'Content-Type': 'application/json', ...(init?.headers ?? {}) },
  })
  if (!res.ok) {
    // The agent answers {"error": "..."} for every failure, so surface that
    // rather than a bare status code the user cannot act on.
    const body = await res.json().catch(() => null)
    throw new ApiError(body?.error ?? `request failed (${res.status})`)
  }
  return res.json() as Promise<T>
}

export const api = {
  server: () => request<ServerInfo>('/api/v1/server'),

  claim: (name: string, password: string) =>
    request<{ token: string; user: User }>('/api/v1/setup', {
      method: 'POST',
      body: JSON.stringify({ name, password }),
    }),

  login: (name: string, password: string) =>
    request<{ token: string; user: User }>('/api/v1/login', {
      method: 'POST',
      body: JSON.stringify({ name, password }),
    }),
}
