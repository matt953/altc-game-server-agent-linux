const KEY = 'altc.token'

/**
 * Kept in localStorage so a refresh does not sign you out. The token is the
 * same credential the API uses everywhere; this is only where the browser
 * keeps its copy.
 */
export const session = {
  token: () => localStorage.getItem(KEY),
  save: (token: string) => localStorage.setItem(KEY, token),
  clear: () => localStorage.removeItem(KEY),
}
