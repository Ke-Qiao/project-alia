import type { TokenResponse, UserInfo } from '@/lib/api/generated/types.gen';

export interface Session {
  accessToken: string,
  refreshToken: string,
  user: UserInfo
}

/** `undefined` means the browser storage has not been read yet (server render, hydration). */
export type SessionSnapshot = Session | null | undefined;

export type SignOutReason = 'manual' | 'expired';

export type SessionStorage = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>;

export const SESSION_STORAGE_KEY = 'alia.session';

function isString(value: unknown): value is string {
  return typeof value === 'string' && value.length > 0;
}

function parseSession(raw: string | null): Session | null {
  if (!raw) return null;
  try {
    const value: unknown = JSON.parse(raw);
    if (typeof value !== 'object' || value === null) return null;
    const { accessToken, refreshToken, user } = value as Record<string, unknown>;
    if (typeof user !== 'object' || user === null || !isString(accessToken) || !isString(refreshToken)) return null;
    const { id, email, name, role, status } = user as Record<string, unknown>;
    if (typeof name !== 'string' || !isString(id) || !isString(email) || !isString(role) || !isString(status)) return null;
    return { accessToken, refreshToken, user: { id, email, name, role, status } };
  } catch {
    return null;
  }
}

/**
 * Holds the signed-in session in web storage so that every tab shares one
 * refresh token. The server revokes a refresh token when it is used, so two
 * tabs holding different copies would sign each other out.
 */
export function createSessionStore(getStorage: () => SessionStorage | null) {
  const listeners = new Set<() => void>();
  let cachedRaw: string | null | undefined;
  let cached: Session | null = null;
  // Used when web storage is unavailable (blocked by the browser).
  let memory: string | null = null;
  let signOutReason: SignOutReason | null = null;

  function readRaw(): string | null {
    try {
      const storage = getStorage();
      return storage ? storage.getItem(SESSION_STORAGE_KEY) : memory;
    } catch {
      return memory;
    }
  }

  function writeRaw(raw: string | null) {
    memory = raw;
    try {
      const storage = getStorage();
      if (raw === null) storage?.removeItem(SESSION_STORAGE_KEY);
      else storage?.setItem(SESSION_STORAGE_KEY, raw);
    } catch {
      // Keep the in-memory copy.
    }
  }

  function notify() {
    listeners.forEach(listener => listener());
  }

  /** Returns a referentially stable value for as long as the stored session is unchanged. */
  function get(): Session | null {
    const raw = readRaw();
    if (raw !== cachedRaw) {
      cachedRaw = raw;
      cached = parseSession(raw);
    }
    return cached;
  }

  return {
    get,
    set(session: Session) {
      signOutReason = null;
      writeRaw(JSON.stringify(session));
      notify();
    },
    /**
     * Stores rotated tokens only if the session they were issued for is still
     * the current one. A refresh that completes after sign-out, or after
     * another account signed in, must not bring the old session back.
     */
    updateTokens(usedRefreshToken: string, tokens: TokenResponse): boolean {
      const current = get();
      if (current?.refreshToken !== usedRefreshToken) return false;
      writeRaw(JSON.stringify({
        accessToken: tokens.access_token,
        refreshToken: tokens.refresh_token,
        user: current.user
      } satisfies Session));
      notify();
      return true;
    },
    clear(reason: SignOutReason) {
      signOutReason = reason;
      writeRaw(null);
      notify();
    },
    /** Why the last session in this tab ended; reset by the next sign-in. */
    getSignOutReason(): SignOutReason | null {
      return signOutReason;
    },
    /** Call when another tab changed the stored session. */
    notify,
    subscribe(listener: () => void): () => void {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    }
  };
}

export type SessionStore = ReturnType<typeof createSessionStore>;
