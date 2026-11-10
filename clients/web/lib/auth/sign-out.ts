import type { SessionStore } from './session-store';

/** Clears local access immediately; server revocation must never delay sign-out. */
export async function signOut(
  store: Pick<SessionStore, 'get' | 'clear'>,
  requestLogout: (refreshToken: string) => Promise<unknown>
): Promise<void> {
  const refreshToken = store.get()?.refreshToken;
  store.clear('manual');
  if (!refreshToken) return;

  try {
    await requestLogout(refreshToken);
  } catch {
    // The local session stays cleared even when the server cannot be reached.
  }
}
