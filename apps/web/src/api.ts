export interface Viewer { id: string; username: string; personal_tenant_id: string; server_admin: boolean }
export interface WebConfig { registration_enabled: boolean; github_enabled?:boolean; version: string }
export class ApiError extends Error {
  constructor(public status: number, public code: string) { super(code); }
}
export async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`/api/web${path}`, {
    credentials: 'same-origin', ...init,
    headers: { 'Content-Type': 'application/json', ...init?.headers },
  });
  if (!response.ok) {
    const error = await response.json().catch(() => ({ code: 'server_error' }));
    if (response.status === 401 && !['/session', '/register'].includes(path)) {
      window.dispatchEvent(new Event('pab-session-expired'));
    }
    throw new ApiError(response.status, error.code ?? 'invalid_input');
  }
  return response.status === 204 ? undefined as T : response.json();
}
export const post = <T,>(path: string, body?: unknown) => api<T>(path, { method: 'POST', body: JSON.stringify(body ?? {}) });
