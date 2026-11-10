import type { AuthResponse, TokenResponse, UserResponse } from '@/lib/api/generated/types.gen';

import type { MockAccount, MockDataset } from './dataset';

/** Everything the mock host changes at runtime. Persisted so a page reload keeps sessions valid. */
export interface MockState {
  accounts: MockAccount[],
  /** token → user id */
  accessTokens: Record<string, string>,
  refreshTokens: Record<string, string>
}

export interface MockPersistence {
  load: () => MockState | null,
  save: (state: MockState) => void
}

export interface MockHostOptions {
  /** Origin the app believes it is talking to; requests elsewhere are not intercepted. */
  origin: string,
  persistence?: MockPersistence
}

function errorResponse(status: number, code: string, message: string): Response {
  // Same body shape as AppError::into_response in packages/alia-server/src/error.rs.
  return Response.json({ error: { code, message } }, { status });
}

function unauthorized(message: string) {
  return errorResponse(401, 'AUTH_INVALID_CREDENTIALS', message);
}

function invalid(message: string) {
  return errorResponse(400, 'VALIDATION_ERROR', message);
}

function field(body: unknown, name: string): string {
  if (typeof body !== 'object' || body === null) return '';
  const value = (body as Record<string, unknown>)[name];
  return typeof value === 'string' ? value : '';
}

/**
 * An in-memory stand-in for alia-server, used as the API client's `fetch`
 * when mock mode is on. It implements only the routes the web app calls and
 * mirrors their status codes and messages (packages/alia-server/src/api/auth.rs,
 * users.rs, health.rs, auth/middleware.rs). Anything else answers 404.
 */
export function createMockHost(dataset: MockDataset, options: MockHostOptions) {
  const { origin, persistence } = options;
  const state: MockState = persistence?.load() ?? {
    accounts: structuredClone(dataset.accounts),
    accessTokens: {},
    refreshTokens: {}
  };
  let offline = false;

  function save() {
    persistence?.save(state);
  }

  function issueTokens(userId: string): TokenResponse {
    const tokens = {
      access_token: `mock-access-${crypto.randomUUID()}`,
      refresh_token: `mock-refresh-${crypto.randomUUID()}`
    };
    state.accessTokens[tokens.access_token] = userId;
    state.refreshTokens[tokens.refresh_token] = userId;
    return tokens;
  }

  function authResponse(user: UserResponse): AuthResponse {
    const { id, email, name, role, status } = user;
    return { ...issueTokens(id), user: { id, email, name, role, status } };
  }

  function login(body: unknown): Response {
    const email = field(body, 'email').trim().toLowerCase();
    if (!email.includes('@')) return invalid('invalid email address');
    const account = state.accounts.find(item => item.user.email === email && item.user.status === 'active');
    if (account?.password !== field(body, 'password')) return unauthorized('invalid email or password');
    account.user.last_login_at = new Date().toISOString();
    return Response.json(authResponse(account.user));
  }

  function register(body: unknown): Response {
    const email = field(body, 'email').trim().toLowerCase();
    if (email.length > 254 || !email.includes('@')) return invalid('invalid email address');
    const name = field(body, 'name').trim();
    if (name.length === 0 || name.length > 120) return invalid('name must be between 1 and 120 characters');
    const password = field(body, 'password');
    if (password.length < 8) return invalid('password must be at least 8 characters');
    if (state.accounts.some(item => item.user.email === email)) return invalid('email already registered');

    const now = new Date().toISOString();
    const account: MockAccount = {
      password,
      user: {
        id: crypto.randomUUID(),
        email,
        name,
        role: state.accounts.length === 0 ? 'admin' : 'user',
        status: 'active',
        created_at: now,
        updated_at: now,
        last_login_at: now
      }
    };
    state.accounts.push(account);
    return Response.json(authResponse(account.user));
  }

  function refresh(body: unknown): Response {
    const token = field(body, 'refresh_token').trim();
    // The server revokes a refresh token when it is used.
    const { [token]: userId, ...remaining } = state.refreshTokens;
    state.refreshTokens = remaining;
    const account = state.accounts.find(item => item.user.id === userId);
    if (!account) return unauthorized('invalid refresh token');
    if (account.user.status !== 'active') return unauthorized('user is not active');
    return Response.json(issueTokens(account.user.id));
  }

  function me(request: Request): Response {
    const header = request.headers.get('Authorization');
    if (!header) return unauthorized('Missing authorization header');
    if (!header.startsWith('Bearer ')) return unauthorized('Invalid authorization format');
    const userId = state.accessTokens[header.slice('Bearer '.length)];
    if (!userId) return unauthorized('Invalid or expired token');
    const account = state.accounts.find(item => item.user.id === userId);
    if (!account) return unauthorized('Authenticated user no longer exists');
    if (account.user.status !== 'active') return unauthorized('Authenticated user is not active');
    return Response.json(account.user);
  }

  async function route(request: Request): Promise<Response> {
    const { pathname } = new URL(request.url);
    const key = `${request.method} ${pathname}`;
    const body: unknown = request.method === 'POST' ? await request.json().catch(() => null) : null;

    switch (key) {
      case 'GET /health': return Response.json(dataset.health);
      case 'POST /api/v1/auth/login': return login(body);
      case 'POST /api/v1/auth/register': return register(body);
      case 'POST /api/v1/auth/refresh': return refresh(body);
      case 'GET /api/v1/users/me': return me(request);
      default: return errorResponse(404, 'NOT_FOUND', `mock host does not implement ${key}`);
    }
  }

  const mockFetch: typeof fetch = async (input, init) => {
    const request = new Request(input, init);
    if (new URL(request.url).origin !== origin) return fetch(request);

    if (offline) throw new TypeError('Failed to fetch (mock host offline)');

    const response = await route(request);
    save();
    return response;
  };

  return {
    fetch: mockFetch,
    /** Scenario switches for debugging; exposed in the browser as `window.aliaMock`. */
    controls: {
      /** Every issued access token stops working; refresh tokens stay valid. */
      expireAccessTokens() {
        state.accessTokens = {};
        save();
      },
      /** Access and refresh tokens stop working, as when the session is revoked server-side. */
      revokeSessions() {
        state.accessTokens = {};
        state.refreshTokens = {};
        save();
      },
      /** Requests fail as if the server were unreachable. */
      setOffline(value: boolean) {
        offline = value;
      },
      /** Back to the seed dataset. */
      reset() {
        state.accounts = structuredClone(dataset.accounts);
        state.accessTokens = {};
        state.refreshTokens = {};
        save();
      }
    }
  };
}

export type MockHost = ReturnType<typeof createMockHost>;
