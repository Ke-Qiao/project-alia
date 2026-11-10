'use client';

import { useSyncExternalStore } from 'react';

import { createSessionStore, SESSION_STORAGE_KEY } from './session-store';
import type { SessionSnapshot } from './session-store';

export const sessionStore = createSessionStore(() => (typeof window === 'undefined' ? null : window.localStorage));

function subscribe(listener: () => void) {
  const unsubscribe = sessionStore.subscribe(listener);
  // Fired only for changes made by other tabs.
  const onStorage = (event: StorageEvent) => {
    if (event.key === SESSION_STORAGE_KEY || event.key === null) listener();
  };
  window.addEventListener('storage', onStorage);
  return () => {
    unsubscribe();
    window.removeEventListener('storage', onStorage);
  };
}

function getServerSnapshot(): SessionSnapshot {
  return undefined;
}

/** `undefined` while hydrating, `null` when signed out. */
export function useSession(): SessionSnapshot {
  return useSyncExternalStore(subscribe, sessionStore.get, getServerSnapshot);
}

/** Serializes token refreshes across tabs where the Web Locks API exists. */
export function runRefreshExclusive<T>(task: () => Promise<T>): Promise<T> {
  if (typeof navigator === 'undefined' || !('locks' in navigator)) return task();
  return navigator.locks.request('alia.session.refresh', task);
}
