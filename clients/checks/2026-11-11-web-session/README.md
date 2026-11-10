# Web account session — 2026-11-11

Starting commit: `016b9b2e52710affad91807440d583903725a109` (`codex/frontend-infra-fixes`).
Branch: `codex/web-account-session`. Scope: `clients/web` only. No backend,
iOS or Android change; nothing pushed or deployed.

## What was built

The first usable loop: register or sign in → signed-in frame (sidebar on
desktop, bottom tab bar on small screens) → overview (`GET /health`) and
account (`GET /api/v1/users/me`) → silent token refresh → sign out or
expired-session return to `/login`. Details are in [clients/README.md](../../README.md).

Why this loop first: every route except `/health` and `/auth/*` needs a bearer
token, authentication is the first acceptance item of D01 in
`docs/refactor/deferred.md`, and the server has no Web text-chat channel yet
(`channels/` holds only `cli` and `embedded`), so a chat page could not be
closed end to end.

## Cases checked before writing code

| Case | Handling | Evidence |
| --- | --- | --- |
| API unreachable, blocked, or not an Alia server | `ApiError` with status 0 / `INVALID_RESPONSE`; pages show an error with 重试 in place of the data, never an empty list; SWR retries only network and 5xx errors | test 9; browser: overview and account with requests failing |
| 401 means three different things | sign-in 401 → field error; bearer 401 → refresh once; refresh 401 → session ends as expired; 403 → "没有权限", session kept | tests 1, 3, 4; browser: wrong password, `expireAccessTokens()`, `revokeSessions()` |
| Wrong client or wrong context | hooks only from `@/lib/data`; the one direct SDK call (`refresh`) passes the provider's client | code review; mock mode intercepts by API origin, and all page data came from it |
| OpenAPI gaps | `logout` and the error body are undeclared. No hand-written types: sign-out is local only, the error body is parsed defensively | recorded under "Not done" |
| Double submit, races | submit guarded by a ref and the button's loading state; one refresh shared by concurrent 401s and across tabs; a refresh finishing after sign-out does not restore the session | tests 3, 6, 7; browser: double click issued one token pair |

## Results

| Check | Result | Evidence |
| --- | --- | --- |
| `pnpm web:lint` | passed, 0 problems. The first run had 24 errors, all fixed | [final](web-lint-final.log), [first run](web-lint-first.log) |
| `pnpm web:typecheck` | passed | [log](web-typecheck-final.log) |
| `pnpm web:test` (Node 24), 9 tests | 9 passed | [log](web-test.log) |
| `pnpm web:build` | passed; `/`, `/account`, `/login`, `/register` prerendered | [log](web-build.log) |
| coss sources match the registry | 9 files byte-identical (SHA-256) | compared in the sandbox |
| Browser, mock mode, 375×812 and 1200×760 | passed: validation, wrong password, sign-in, register (duplicate and new), keyboard submit, double click, silent refresh, expired session, offline with stale data and retry, sign-out, navigation, `aria-current` | built-in browser against `pnpm web:dev`; no saved recording |
| Real alia-server integration | **not run** | — |

The four `pnpm` commands ran on the developer's Mac (Node 24.15.0); the
development sandbox is Linux without network, pnpm or cargo and cannot load
the macOS binaries ESLint and Next.js need. Lint ran last; two test/mock lines
changed after the logged typecheck, test and build runs (`.catch` in the test
file, one destructuring in `lib/mock/host.ts`), and were re-checked with `tsc`
and the same 9 tests in the sandbox. All logs, including intermediate runs,
are in `run/web-session-20261111/` on this machine.

The overview page calls only the public `/health`, so a revoked session is
noticed on the next authenticated request (opening 账号), not while idling on 概览.

## Not done

- **Real backend integration.** The browser pane blocked requests from the page
  to `http://127.0.0.1:3000` (`net::ERR_BLOCKED_BY_CLIENT`), and no server was
  started this round. Nothing here shows the pages working against alia-server.
- **Server-side sign-out.** Needs `#[utoipa::path]` on `logout` in
  `packages/alia-server/src/api/auth.rs`, its registration in `openapi.rs`,
  then `pnpm web:openapi`.
- Validation messages for empty or malformed fields are the browser's own and
  follow the browser language.
- No password reset, profile editing or user administration.

## Deviations

- The first nine coss components were written from the registry JSON fetched in
  a browser, with imports rewritten the way the CLI does, and verified by
  SHA-256 against the registry. `sidebar` and its dependencies were added with
  `pnpm --dir clients/web ui:add @coss/sidebar`.
- A mock mode was added at the user's request during this round; the original
  brief excluded mocks.
