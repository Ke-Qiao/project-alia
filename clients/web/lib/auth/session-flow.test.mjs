// Run with Node 24 (type stripping): pnpm --dir clients/web test
// Exercises the real session store, token refresher and auth fetch against the mock host.
import assert from 'node:assert/strict';
import { test } from 'node:test';

import { ApiError, describeApiError, isRetryableError, toApiError } from '../api-error.ts';
import { mockDataset } from '../mock/dataset.ts';
import { createMockHost } from '../mock/host.ts';
import { createSessionStore, SESSION_STORAGE_KEY } from './session-store.ts';
import { createAuthFetch, createTokenRefresher } from './token-refresh.ts';

const ORIGIN = 'http://alia.test';
const [admin] = mockDataset.accounts;
const rUnreachable = /无法连接/;
const rNotAlia = /无法识别的响应/;
const rForbidden = /没有权限/;

function memoryStorage() {
  const map = new Map();
  return {
    getItem: key => map.get(key) ?? null,
    setItem(key, value) {
      map.set(key, value);
    },
    removeItem(key) {
      map.delete(key);
    }
  };
}

async function api(fetchImpl, method, path, { body, token } = {}) {
  const headers = new Headers();
  if (body) headers.set('Content-Type', 'application/json');
  if (token) headers.set('Authorization', `Bearer ${token}`);
  let response;
  try {
    response = await fetchImpl(ORIGIN + path, { method, headers, body: body && JSON.stringify(body) });
  } catch (error) {
    throw toApiError(error, undefined);
  }
  const payload = await response.json();
  if (!response.ok) throw toApiError(payload, response);
  return payload;
}

/** One "browser tab": its own refresher, sharing `storage` and the server with other tabs. */
function createTab(host, storage, runExclusive) {
  const store = createSessionStore(() => storage);
  const calls = { refresh: 0 };

  const refreshAccessToken = createTokenRefresher(store, (refreshToken) => {
    calls.refresh += 1;
    return api(host.fetch, 'POST', '/api/v1/auth/refresh', { body: { refresh_token: refreshToken } });
  }, runExclusive);
  const authFetch = createAuthFetch(refreshAccessToken, host.fetch);

  return {
    store,
    calls,
    async signIn(email = admin.user.email, password = admin.password) {
      const data = await api(authFetch, 'POST', '/api/v1/auth/login', { body: { email, password } });
      store.set({ accessToken: data.access_token, refreshToken: data.refresh_token, user: data.user });
    },
    me: () => api(authFetch, 'GET', '/api/v1/users/me', { token: store.get()?.accessToken })
  };
}

function setup() {
  const host = createMockHost(mockDataset, { origin: ORIGIN });
  return { host, tab: createTab(host, memoryStorage()) };
}

test('wrong password is a 401 on sign-in and does not touch the session or try to refresh', async () => {
  const { tab } = setup();
  await assert.rejects(tab.signIn(admin.user.email, 'wrong-password'), error => error instanceof ApiError && error.status === 401);
  assert.equal(tab.store.get(), null);
  assert.equal(tab.calls.refresh, 0);
  assert.equal(tab.store.getSignOutReason(), null);
});

test('a disabled account cannot sign in', async () => {
  const { tab } = setup();
  const disabled = mockDataset.accounts.find(account => account.user.status === 'disabled');
  await assert.rejects(tab.signIn(disabled.user.email, disabled.password), error => error.status === 401);
});

test('expired access token: concurrent requests share one refresh and all succeed', async () => {
  const { host, tab } = setup();
  await tab.signIn();
  const before = tab.store.get();
  host.controls.expireAccessTokens();

  const results = await Promise.all([tab.me(), tab.me(), tab.me()]);

  assert.deepEqual(results.map(user => user.email), [admin.user.email, admin.user.email, admin.user.email]);
  assert.equal(tab.calls.refresh, 1);
  assert.notEqual(tab.store.get().refreshToken, before.refreshToken);
});

test('revoked session: the request fails with 401 and the session ends as expired', async () => {
  const { host, tab } = setup();
  await tab.signIn();
  host.controls.revokeSessions();

  await assert.rejects(tab.me(), error => error instanceof ApiError && error.status === 401);
  assert.equal(tab.store.get(), null);
  assert.equal(tab.store.getSignOutReason(), 'expired');
});

test('server unreachable during refresh: the session is kept and the error is a network error', async () => {
  const host = createMockHost(mockDataset, { origin: ORIGIN });
  const tab = createTab(host, memoryStorage());
  await tab.signIn();
  host.controls.expireAccessTokens();
  // The data request still gets its 401; the refresh that follows cannot reach the server.
  const realFetch = host.fetch;
  host.fetch = (input, init) => {
    const url = typeof input === 'string' ? input : input.url;
    if (url.endsWith('/auth/refresh')) return Promise.reject(new TypeError('Failed to fetch'));
    return realFetch(input, init);
  };
  const storage = memoryStorage();
  storage.setItem(SESSION_STORAGE_KEY, JSON.stringify(tab.store.get()));
  const offlineTab = createTab(host, storage);

  await assert.rejects(offlineTab.me(), error => error instanceof ApiError && error.status === 0 && isRetryableError(error));
  assert.notEqual(offlineTab.store.get(), null);
});

test('sign-out while a refresh is in flight does not bring the session back', async () => {
  const { host, tab } = setup();
  await tab.signIn();
  host.controls.expireAccessTokens();

  const pending = tab.me();
  tab.store.clear('manual');

  await assert.rejects(pending, error => error.status === 401);
  assert.equal(tab.store.get(), null);
  assert.equal(tab.store.getSignOutReason(), 'manual');
});

test('two tabs refreshing the same token: neither is signed out', async () => {
  const host = createMockHost(mockDataset, { origin: ORIGIN });
  const storage = memoryStorage();
  // Stand-in for the Web Locks API: one refresh at a time across tabs.
  let queue = Promise.resolve();
  const runExclusive = (task) => {
    const result = queue.then(task);
    queue = result.catch(Boolean);
    return result;
  };
  const first = createTab(host, storage, runExclusive);
  const second = createTab(host, storage, runExclusive);
  await first.signIn();
  host.controls.expireAccessTokens();

  const users = await Promise.all([first.me(), second.me()]);

  assert.equal(users.length, 2);
  assert.equal(first.calls.refresh + second.calls.refresh, 1);
  assert.notEqual(second.store.get(), null);
});

test('corrupt stored session reads as signed out', () => {
  const storage = memoryStorage();
  const store = createSessionStore(() => storage);
  storage.setItem(SESSION_STORAGE_KEY, '{"accessToken":1}');
  assert.equal(store.get(), null);
  storage.setItem(SESSION_STORAGE_KEY, 'not json');
  assert.equal(store.get(), null);
});

test('errors are classified so that failures are not shown as empty data', () => {
  const network = toApiError(new TypeError('Failed to fetch'), undefined);
  assert.equal(network.status, 0);
  assert.match(describeApiError(network, ORIGIN), rUnreachable);

  const notAlia = toApiError(new SyntaxError('Unexpected token <'), { ok: true, status: 200, statusText: 'OK' });
  assert.match(describeApiError(notAlia, ORIGIN), rNotAlia);

  const forbidden = toApiError({ error: { code: 'AUTH_FORBIDDEN', message: 'insufficient permissions' } }, { ok: false, status: 403, statusText: 'Forbidden' });
  assert.equal(forbidden.code, 'AUTH_FORBIDDEN');
  assert.match(describeApiError(forbidden, ORIGIN), rForbidden);
  assert.equal(isRetryableError(forbidden), false);

  const html = toApiError('<html>Bad Gateway</html>', { ok: false, status: 502, statusText: 'Bad Gateway' });
  assert.equal(isRetryableError(html), true);
});
