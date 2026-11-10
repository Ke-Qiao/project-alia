import { aliaApiUrl } from '@/lib/api-url';

import { mockDataset } from './dataset';
import { createMockHost } from './host';
import type { MockHost, MockState } from './host';

/**
 * Mock mode replaces the network with an in-browser stand-in for alia-server.
 * It is a debugging aid: nothing shown in this mode comes from a real server.
 */
export const mockEnabled = process.env.NEXT_PUBLIC_ALIA_MOCK === '1';

const STATE_KEY = 'alia.mock.state';

declare global {
  interface Window {
    aliaMock?: MockHost['controls']
  }
}

export function createBrowserMockHost(): MockHost {
  const host = createMockHost(mockDataset, {
    origin: new URL(aliaApiUrl).origin,
    persistence: {
      load() {
        try {
          const raw = window.localStorage.getItem(STATE_KEY);
          return raw ? JSON.parse(raw) as MockState : null;
        } catch {
          return null;
        }
      },
      save(state) {
        try {
          window.localStorage.setItem(STATE_KEY, JSON.stringify(state));
        } catch {
          // The mock still works for this page load.
        }
      }
    }
  });
  window.aliaMock = host.controls;
  return host;
}
