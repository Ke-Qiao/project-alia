# Frontend infrastructure fixes — 2026-11-11

Starting commit: `70c180fcc4157ecfcd99a9237327d6b24256999e`.
Branch: `codex/frontend-infra-fixes`.

The four acceptance failures are fixed. Minimum supported versions are now
**iOS 18.0** and **Android 13 / API 33**. Android compile/target SDK remain 37.
No business pages, device actions, deployment, or backend runtime were added.

| Check | Result | Evidence |
| --- | --- | --- |
| Existing Cargo default entry | Cargo selected `alia-server`; verification runner did not execute it | [log](backend-default-target.log) |
| Clean Web frozen-lockfile install | Passed, pnpm 10.28.1 / Node 24.15.0 | [log](web-frozen-install-online.log) |
| Web ESLint / types | Passed after separating hooks from the React component for Fast Refresh | [lint](web-lint-final.log), [types](web-typecheck-final.log) |
| Web production build | Passed on a fresh source copy | [log](web-build.log) |
| OpenAPI export + Hey API | Root command passed; spec and generated files unchanged | [log](openapi-generation.log), [comparison](openapi-unchanged.log) |
| Same-instance Tayori hooks | Passed for unset, empty and custom API origins; no API requests made | [default](web-hooks-default.log), [empty](web-hooks-empty.log), [custom](web-hooks-custom.log) |
| coss first component | Pinned CLI added button/spinner in a temporary copy; TypeScript passed | [add](coss-add-button.log), [types](coss-component-typecheck.log) |
| Web default port | HTTP 200 on 3100 while backend port 3000 remained available | [log](web-dev-port.log) |
| iOS Simulator SDK build | Passed, signing disabled; built minimum OS is 18.0 | [build](ios-build.log), [metadata](ios-artifact.json) |
| Android debug build | Passed with app dependency lockfile; APK min 33 / target 37 | [build](android-build.log), [metadata](android-artifact.log) |
| Android API compatibility lint | Passed: 0 errors, 1 application-icon warning | [log](android-lint-verified-cache.log), [report](android-lint-report.txt) |

`verified-source-sha256.json` records the source files verified against the
build copy. `results.jsonl` records commands, directories, UTC timestamps,
durations and exit codes. The checks ran on 2026-11-11 in Asia/Shanghai.
The short verification scripts are retained here as reproducible evidence,
not a new application test framework. `verify-hooks.cjs` accepts the Web
project directory as its only argument. Other scripts retain this session's
temporary paths; inspect those paths before reusing them.

## Environment notes

An initial offline Web install lacked cache entries; the clean online
frozen-lockfile install passed. The initial combined Provider/hooks module
failed the existing React Fast Refresh lint rule; the final code keeps the
factory and hooks in `lib/data.ts`, and the component in
`lib/data-provider.tsx`, with the root layout updated.

Gradle's Java HTTP client repeatedly failed TLS handshakes for three Google
lint JARs. They were downloaded from the **same official HTTPS URLs** with the
system client and checked against Google's published SHA-1 checksums; SHA-256
values were also recorded in `lint-dependency-fetch.log`. Because Gradle had
not registered manually downloaded artifacts in its metadata cache, the final
lint command used a temporary init script pointing only those three modules
to a local file repository. No project repository, certificate validation or
TLS setting changed. The APK build passed without that init script. Full
attempt logs remain in `run/frontend-infra-fixes-20261111/` on this machine.

The lone lint warning is the original shell's missing application icon. No
launcher artwork was invented for this infrastructure repair. No unavailable
Android API errors were reported for minSdk 33.

## Scope of the result

These results establish compilation, configuration, generation, local Web HTTP
startup, and targeted integration checks. No iOS/Android simulator or physical
device was launched. A build with deployment target 18.0 or minSdk 33 is not a
claim that the app was run on those OS versions. The complete server, database,
embedded device chain and production settings were untouched.

SWR 2.6.0-beta.0 and Material3 1.5.0-rc01 remain the documented prerequisites
for the selected Tayori and Expressive APIs. Kotlin's published fully supported
AGP matrix stops below AGP 9.5.1; the configured combination passed these actual
builds. No unrelated compiler or library upgrade was made.
