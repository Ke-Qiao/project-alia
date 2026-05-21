import { isLocale, type Locale } from '@/lib/i18n';
import { deleteStoredValue, getStoredValue, setStoredValue } from '@/lib/session/storage';
import * as React from 'react';
import { Platform } from 'react-native';
import { mutate } from 'swr';

const SESSION_KEY = 'alia.session.v1';

export type SessionUser = {
  id: string;
  email: string;
  name: string;
  role: string;
  status: string;
};

type PersistedSession = {
  baseUrl: string | null;
  accessToken: string | null;
  refreshToken: string | null;
  user: SessionUser | null;
  locale: Locale;
};

type SessionContextValue = PersistedSession & {
  hydrated: boolean;
  effectiveBaseUrl: string | null;
  setServerBaseUrl: (baseUrl: string | null) => Promise<void>;
  setAuth: (auth: { accessToken: string; refreshToken: string; user: SessionUser }) => Promise<void>;
  setLocale: (locale: Locale) => Promise<void>;
  clearAuth: () => Promise<void>;
  resetConnection: () => Promise<void>;
};

const defaultSession: PersistedSession = {
  baseUrl: Platform.OS === 'web' ? '' : null,
  accessToken: null,
  refreshToken: null,
  user: null,
  locale: 'en',
};

const SessionContext = React.createContext<SessionContextValue | null>(null);

function normalizeBaseUrl(input: string | null) {
  const value = input?.trim();
  if (!value) {
    return Platform.OS === 'web' ? '' : null;
  }
  return value.replace(/\/+$/, '');
}

export function SessionProvider({ children }: { children: React.ReactNode }) {
  const [session, setSession] = React.useState<PersistedSession>(defaultSession);
  const [hydrated, setHydrated] = React.useState(false);

  React.useEffect(() => {
    let cancelled = false;
    getStoredValue(SESSION_KEY)
      .then((raw) => {
        if (cancelled) return;
        if (!raw) {
          setSession(defaultSession);
          return;
        }
        const parsed = JSON.parse(raw) as Partial<PersistedSession>;
        setSession({
          baseUrl: normalizeBaseUrl(parsed.baseUrl ?? defaultSession.baseUrl),
          accessToken: parsed.accessToken ?? null,
          refreshToken: parsed.refreshToken ?? null,
          user: parsed.user ?? null,
          locale: isLocale(parsed.locale) ? parsed.locale : 'en',
        });
      })
      .catch(() => setSession(defaultSession))
      .finally(() => {
        if (!cancelled) setHydrated(true);
      });

    return () => {
      cancelled = true;
    };
  }, []);

  const persist = React.useCallback(async (next: PersistedSession) => {
    setSession(next);
    await setStoredValue(SESSION_KEY, JSON.stringify(next));
    await mutate(() => true, undefined, { revalidate: false });
  }, []);

  const value = React.useMemo<SessionContextValue>(
    () => ({
      ...session,
      hydrated,
      effectiveBaseUrl: session.baseUrl ?? (Platform.OS === 'web' ? '' : null),
      setServerBaseUrl: async (baseUrl) => {
        await persist({
          ...session,
          baseUrl: normalizeBaseUrl(baseUrl),
          accessToken: null,
          refreshToken: null,
          user: null,
        });
      },
      setAuth: async (auth) => {
        await persist({
          ...session,
          accessToken: auth.accessToken,
          refreshToken: auth.refreshToken,
          user: auth.user,
        });
      },
      setLocale: async (locale) => {
        await persist({ ...session, locale });
      },
      clearAuth: async () => {
        await persist({
          ...session,
          accessToken: null,
          refreshToken: null,
          user: null,
        });
      },
      resetConnection: async () => {
        const next = { ...defaultSession, locale: session.locale };
        setSession(next);
        await deleteStoredValue(SESSION_KEY);
        await mutate(() => true, undefined, { revalidate: false });
      },
    }),
    [hydrated, persist, session]
  );

  return <SessionContext.Provider value={value}>{children}</SessionContext.Provider>;
}

export function useSession() {
  const ctx = React.useContext(SessionContext);
  if (!ctx) {
    throw new Error('useSession must be used inside SessionProvider');
  }
  return ctx;
}
