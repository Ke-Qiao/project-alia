# Clients

Three separate shells. There is no shared UI layer and no business screen.

Root scripts in `package.json` forward to these projects. The server's Cargo
`default-run` remains `alia-server`; adding the OpenAPI exporter does not change
`pnpm server`, `pnpm dev:backend`, or `mise run dev-server`.

## Web — `clients/web`

| Tool | Version |
| --- | --- |
| Node | 24.15.0 (mise) |
| pnpm | 10.28.1 |
| Next.js | 16.4.5 |
| React | 19.3.1 |
| TypeScript | 6.0.3 |
| Tailwind CSS | 4.3.5 |
| ESLint | 10.14.0 |
| eslint-config-sukka | 9.2.1 |
| SWR | 2.6.0-beta.0 |
| tayori | 0.4.1 |
| @hey-api/openapi-ts | 0.99.0 |
| shadcn CLI | 4.23.0 |

`tayori@0.4.1` requires SWR `^2.6.0-beta.0`. The newest stable SWR is 2.5.1, so the web app uses the 2.6 beta. Tayori is the Hey API data layer on top of SWR, not a news library.

TypeScript is 6.0.3, not 7.0.2. `eslint-config-sukka@9.2.1` loads `typescript-eslint@8.72`, whose peer range stops before 6.1.0, so 7.0.2 cannot lint this project.

`@hey-api/client-fetch` is not installed. Since openapi-ts 0.73 the Fetch client is bundled; the plugin name is still `@hey-api/client-fetch`. The input path is `./openapi/openapi.json`. A path with one slash and no leading `./` is parsed as a Hey API registry id and generation stops before reading the file.

coss ui is a copy-in component set. The shell includes the neutral theme, base
styles, dark variant, animation utilities, `cn` helper, and Base UI/CVA/icon
dependencies. `components.json` uses the valid `base-nova` CLI style; coss
components are selected explicitly with `@coss/...`. No component or showcase
page ships in the shell, and the build does not download Google fonts.

```bash
pnpm --dir clients/web install --frozen-lockfile
pnpm web:dev
pnpm web:lint
pnpm web:typecheck
pnpm web:build
pnpm web:openapi
```

Web development runs at `http://localhost:3100`. The server keeps HTTP port
3000 and its default gRPC port 3001, so either process can start first.
When a component is needed, use the pinned CLI, for example:

```bash
pnpm --dir clients/web ui:add @coss/button
```

`pnpm web:openapi` runs `cargo run -p alia-server --bin openapi`, which serializes `ApiDoc` and does not start the server, read config, or connect to a database. It writes `clients/web/openapi/openapi.json`, then Hey API writes `clients/web/lib/api/generated`. Commit both. Do not edit them by hand.

Copy `clients/web/.env.example` to `.env.local` only when you need a non-default API origin. An empty `NEXT_PUBLIC_ALIA_API_URL` still falls back to `http://127.0.0.1:3000`. The example file has no credentials.

ESLint uses `sukka({ ts, react, next })`. `@eslint-sukka/react` is installed separately because sukka loads it at runtime. `node: false` keeps `@types/node` from turning on the Node preset.

Import `useData`, `useDataImmutable`, `useInfinite`, `useMutation`, `usePreload`
and `useClient` from `@/lib/data`. They share the same `tayori()` instance as
the `DataFetchingProvider` mounted by the root layout. Pass a generated SDK
function to these hooks; for a direct SDK call, pass the client returned by
`useClient()` through the SDK's `client` option. Do not create another
`tayori()` instance to consume this provider. The empty entry page makes no
API requests.

## iOS — `clients/ios`

SwiftUI app. Both Debug and Release deployment targets are iOS 18.0. The
compiler and SDK remain Xcode 27.2 / iOS 27.2; lowering the minimum supported
OS does not require an older SDK. No third-party packages are used.

This machine's `xcode-select` points at Command Line Tools. The build command sets `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer` (Xcode 27.2, iPhone SDK 27.2) and does not change that global selection. `CODE_SIGNING_ALLOWED=NO` avoids signing certificates. A successful `pnpm ios:build` is a Simulator SDK build, not a simulator or device launch.

```bash
pnpm ios:build
```

## Android — `clients/android`

| Tool | Version |
| --- | --- |
| AGP | 9.5.1 |
| Gradle | 9.6.0 |
| Kotlin | 2.4.21 |
| JDK | 21 |
| Compose BOM | 2026.09.00 |
| activity-compose | 1.13.0 |
| material3 | 1.5.0-rc01 |
| compileSdk / targetSdk | 37 (Android 17) |
| minSdk | 33 (Android 13) |

Android 13 (API 33) is the minimum supported OS. Compilation and target behavior
remain Android 17 (API 37). AGP 9.5 supports API 37 and uses Gradle 9.6.0 and
Build-Tools 36.0.0. AGP 9 compiles Kotlin itself, so the Kotlin Android plugin
is not applied. The resolved Kotlin and Compose compiler versions are both
2.4.21. This combination builds locally; Kotlin's published fully supported
AGP range for 2.4.21 ends at 9.3.1, so the newer AGP is outside that matrix.

Material 3 Expressive uses `androidx.compose.material3:material3:1.5.0-rc01`.
The RC supplies `MaterialExpressiveTheme`, which is absent from the stable
1.4.0 line. This library choice is for Expressive APIs; Android 17 itself does
not require a prerelease Material library.

`local.properties` is gitignored. Point `sdk.dir` at the Android SDK, or set `ANDROID_HOME`. `compileSdk = 37` uses the package `platforms;android-37.0`; `platforms;android-37` is not a real package id. This machine uses JDK 21 from Zulu, that platform, and Build-Tools 36.0.0.

```bash
# On macOS, select JDK 21 when it is not already the shell default:
export JAVA_HOME=$(/usr/libexec/java_home -v 21)
pnpm android:build
# API compatibility checks, including the Android 13 minimum:
clients/android/gradlew -p clients/android lintDebug
```

`assembleDebug` is a build. It does not install or launch an emulator or a device.

The app's resolved dependencies are tracked in `app/gradle.lockfile`. After an
intentional dependency upgrade, refresh and review the lockfile with:

```bash
clients/android/gradlew -p clients/android :app:dependencies --write-locks
```

Official setup references: [coss](https://coss.com/ui/docs/get-started),
[shadcn manual setup](https://ui.shadcn.com/docs/installation/manual),
[Tayori](https://tayori.skk.moe/),
[Xcode support](https://developer.apple.com/xcode/system-requirements),
[Android 13 / API 33](https://developer.android.com/about/versions/13/setup-sdk),
[AGP 9.5](https://developer.android.com/build/releases/agp-9-5-0-release-notes),
[Kotlin compatibility](https://kotlinlang.org/docs/gradle-configure-project.html).

## Checks

The [2026-11-11 repair checks](checks/2026-11-11-infra-fixes/README.md) verify
the four reported failures and the updated OS minimums. Web installation,
lint, types, generation, build, coss component import and port separation pass.
iOS builds with minimum OS 18.0; the Android APK has minSdk 33 / targetSdk 37.
Android lint reports zero errors and one existing missing-application-icon
warning. Its dependency-download workaround is recorded with the logs; no
project repository or TLS configuration was changed. Neither native app was
launched on a simulator or device.

### Original build records

The following original build logs remain in `clients/checks/` as historical
records for commit `70c180f` (before lowering the minimum OS versions). They
are not simulator launches, device launches, or deployments.

| Check | Result | Log |
| --- | --- | --- |
| OpenAPI export, server not started | Passed. Missing, extra and empty arguments exit 2. A missing parent directory is created. | `openapi-export.log`, `openapi-args.log` |
| Web ESLint | Passed after the PropsWithChildren and config-key fixes | `web-lint.log` |
| Web `tsc --noEmit` | Passed | `web-typecheck.log` |
| Web `next build` | Passed. `/` is a static page. The site is not deployed. | `web-build.log` |
| iOS Simulator SDK | Passed with Xcode 27.2. Built `MinimumOSVersion` is 27.0. Signing was off. The simulator was not launched. | `ios-build.log` |
| Android `assembleDebug` | The first run failed while downloading Maven artifacts (`android-build.log`, TLS handshake to `dl.google.com`). The retry passed and wrote `app-debug.apk`. Nothing was installed on an emulator or a device. | `android-build-2.log` |

The Android SDK used for that APK is `platforms;android-37.0` (`Platform.Version` 17, `AndroidVersion.ApiLevel` 37.0), Build-Tools 36.0.0, and Platform-Tools 37.0.1. AGP installed Platform-Tools during the build. `local.properties` is not committed.
