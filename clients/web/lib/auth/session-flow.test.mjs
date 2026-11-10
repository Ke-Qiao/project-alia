// Run with Node 24 (type stripping): pnpm --dir clients/web test
// Exercises the real session store, token refresher and auth fetch against the mock host.
import assert from 'node:assert/strict';
import { test } from 'node:test';

import { ApiError, describeApiError, isRetryableError, toApiError } from '../api-error.ts';
import { mockDataset } from '../mock/dataset.ts';
import { createMockHost } from '../mock/host.ts';
import { createSessionStore, SESSION_STORAGE_KEY } from './session-store.ts';
import { createAuthFetch, createTokenRefresher } from './token-refresh.ts';
import { signOut } from './sign-out.ts';

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
    me: () => api(authFetch, 'GET', '/api/v1/users/me', { token: store.get()?.accessToken }),
    signOut: () => signOut(store, refreshToken => api(authFetch, 'POST', '/api/v1/auth/logout', { body: { refresh_token: refreshToken } }))
  };
}

function setup() {
  const host = createMockHost(mockDataset, { origin: ORIGIN });
  return { host, tab: createTab(host, memoryStorage()) };
}

test('wrong password is a 401 on sign-in and does not touch the session or try to refresh', async () => {
  const { tab } = setup();
  await assert.rejects(tab.signIn(admin.user.email, 'wrong-password'), error => error instanceof ApiError && error.status === 401 && error.reason === 'invalid_credentials');
  assert.equal(tab.store.get(), null);
  assert.equal(tab.calls.refresh, 0);
  assert.equal(tab.store.getSignOutReason(), null);
});

test('a disabled account cannot sign in', async () => {
  const { tab } = setup();
  const disabled = mockDataset.accounts.find(account => account.user.status === 'disabled');
  await assert.rejects(tab.signIn(disabled.user.email, disabled.password), error => error.status === 401 && error.reason === 'invalid_credentials');
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

test('sign-out after the server rotates tokens but before its response arrives cannot restore the session', { timeout: 5000 }, async () => {
  const host = createMockHost(mockDataset, { origin: ORIGIN });
  const rotated = Promise.withResolvers();
  const release = Promise.withResolvers();
  const realFetch = host.fetch;
  host.fetch = async (input, init) => {
    const response = await realFetch(input, init);
    const url = typeof input === 'string' ? input : input.url;
    if (url.endsWith('/auth/refresh')) {
      rotated.resolve();
      await release.promise;
    }
    return response;
  };
  const tab = createTab(host, memoryStorage());
  await tab.signIn();
  host.controls.expireAccessTokens();

  const pending = tab.me();
  const rejected = assert.rejects(pending, error => error.status === 401);
  await rotated.promise;
  await tab.signOut();
  assert.equal(tab.store.get(), null);
  release.resolve();
  await rejected;
  assert.equal(tab.calls.refresh, 1);
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

test('duplicate email uses its declared status, code and reason', async () => {
  const { host } = setup();
  await assert.rejects(
    api(host.fetch, 'POST', '/api/v1/auth/register', {
      body: { email: admin.user.email, name: 'Another account', password: 'another-password' }
    }),
    error => error instanceof ApiError && error.status === 409 && error.code === 'ALREADY_EXISTS' && error.reason === 'email_already_registered'
  );
});

test('sign-out immediately clears local state and revokes the captured refresh token', async () => {
  const { host, tab } = setup();
  await tab.signIn();
  const oldToken = tab.store.get().refreshToken;
  const pending = tab.signOut();
  assert.equal(tab.store.get(), null);
  assert.equal(tab.store.getSignOutReason(), 'manual');
  await pending;
  await assert.rejects(
    api(host.fetch, 'POST', '/api/v1/auth/refresh', { body: { refresh_token: oldToken } }),
    error => error.status === 401 && error.reason === 'refresh_token_invalid'
  );
});

test('unreachable logout does not prevent local sign-out', async () => {
  const { host, tab } = setup();
  await tab.signIn();
  host.controls.setOffline(true);
  const pending = tab.signOut();
  assert.equal(tab.store.get(), null);
  await pending;
  assert.equal(tab.store.getSignOutReason(), 'manual');
});

test('a late logout failure leaves a newly signed-in account untouched', async () => {
  const { tab } = setup();
  await tab.signIn();
  const previous = tab.store.get();
  const release = Promise.withResolvers();
  let sentToken;
  const pending = signOut(tab.store, (token) => {
    sentToken = token;
    assert.equal(tab.store.get(), null);
    return release.promise;
  });
  assert.equal(tab.store.get(), null);
  assert.equal(sentToken, previous.refreshToken);
  const member = mockDataset.accounts.find(account => account.user.role === 'user' && account.user.status === 'active');
  await tab.signIn(member.user.email, member.password);
  const next = tab.store.get();
  release.reject(new TypeError('Failed to fetch'));
  await pending;
  assert.deepEqual(tab.store.get(), next);
  assert.equal(tab.store.getSignOutReason(), null);
});

test('sign-out without a session never sends an empty token', async () => {
  const { tab } = setup();
  let calls = 0;
  await signOut(tab.store, () => {
    calls += 1;
    return Promise.resolve();
  });
  assert.equal(calls, 0);
  assert.equal(tab.store.get(), null);
});

test('refresh 503 preserves the session regardless of the error reason', async () => {
  const host = createMockHost(mockDataset, { origin: ORIGIN });
  const realFetch = host.fetch;
  host.fetch = (input, init) => {
    const url = typeof input === 'string' ? input : input.url;
    if (url.endsWith('/auth/refresh')) {
      return Promise.resolve(Response.json({ error: {
        code: 'SERVICE_UNAVAILABLE', reason: 'account_verification_unavailable', message: 'Account verification unavailable'
      } }, { status: 503 }));
    }
    return realFetch(input, init);
  };
  const tab = createTab(host, memoryStorage());
  await tab.signIn();
  const before = tab.store.get();
  host.controls.expireAccessTokens();
  await assert.rejects(tab.me(), error => error.status === 503 && error.reason === 'account_verification_unavailable');
  assert.deepEqual(tab.store.get(), before);
  assert.equal(tab.store.getSignOutReason(), null);
});

test('non-Alia and malformed errors cannot invent a field-level reason', () => {
  const response = { ok: false, status: 409, statusText: 'Conflict' };
  for (const payload of [null, 'proxy failure', { error: null }, { error: { code: 1, reason: {}, message: false } }]) {
    const error = toApiError(payload, response);
    assert.equal(error.status, 409);
    assert.equal(error.code, undefined);
    assert.equal(error.reason, undefined);
    assert.equal(describeApiError(error, ORIGIN), typeof payload === 'string' ? payload : 'Conflict');
  }
  const structured = toApiError({ error: {
    code: 'ALREADY_EXISTS', reason: 'email_already_registered', message: 'A different display message'
  } }, response);
  assert.equal(structured.reason, 'email_already_registered');
  assert.equal(describeApiError(structured, ORIGIN), 'A different display message');
});
