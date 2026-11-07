# Clients

Three separate shells. There is no shared UI layer and no business screen.

Root scripts in `package.json` forward to these projects. Backend scripts are unchanged.

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
| @hey-api/client-fetch | 0.13.1 |

`tayori@0.4.1` requires SWR `^2.6.0-beta.0`. The newest stable SWR is 2.5.1, so the web app uses the 2.6 beta. Tayori is the Hey API data layer on top of SWR, not a news library.

TypeScript is 6.0.3, not 7.0.2. `eslint-config-sukka@9.2.1` loads `typescript-eslint@8.72`, whose peer range stops before 6.1.0, so 7.0.2 cannot lint this project.

`@hey-api/client-fetch` is not installed. Since openapi-ts 0.73 the Fetch client is bundled; the plugin name is still `@hey-api/client-fetch`. The input path is `./openapi/openapi.json`. A path with one slash and no leading `./` is parsed as a Hey API registry id and generation stops before reading the file.

coss ui is a copy-in component set. This project only has the `@coss/style` token file (`app/globals.css`) and `components.json`. No component was copied, and the build does not download Google fonts.

```bash
pnpm --dir clients/web install
pnpm web:dev
pnpm web:lint
pnpm web:typecheck
pnpm web:build
pnpm web:openapi
```

`pnpm web:openapi` runs `cargo run -p alia-server --bin openapi`, which serializes `ApiDoc` and does not start the server, read config, or connect to a database. It writes `clients/web/openapi/openapi.json`, then Hey API writes `clients/web/lib/api/generated`. Commit both. Do not edit them by hand.

Copy `clients/web/.env.example` to `.env.local` only when you need a non-default API origin. An empty `NEXT_PUBLIC_ALIA_API_URL` still falls back to `http://127.0.0.1:3000`. The example file has no credentials.

ESLint uses `sukka({ ts, react, next })`. `@eslint-sukka/react` is installed separately because sukka loads it at runtime. `node: false` keeps `@types/node` from turning on the Node preset.

## iOS — `clients/ios`

SwiftUI app. Deployment target is 27.0 and is not lowered. No third-party packages.

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
| compileSdk / targetSdk / minSdk | 37 |

Android 17 is API 37. `minSdk` is also 37: this shell does not claim older devices. AGP 9.5 supports API 37 and defaults to Gradle 9.6.0 and Build-Tools 36.0.0. AGP 9 compiles Kotlin itself, so the Kotlin Android plugin is not applied. The Compose compiler plugin matches Kotlin 2.4.21.

Material 3 Expressive is `androidx.compose.material3:material3:1.5.0-rc01`. The stable 1.4.0 line does not include `MaterialExpressiveTheme`. This is a release candidate required by the Android 17 target, not a stand-in for a missing stable API.

`local.properties` is gitignored. Point `sdk.dir` at the Android SDK, or set `ANDROID_HOME`. `compileSdk = 37` uses the package `platforms;android-37.0`; `platforms;android-37` is not a real package id. This machine uses JDK 21 from Zulu, that platform, and Build-Tools 36.0.0.

```bash
pnpm android:build
```

`assembleDebug` is a build. It does not install or launch an emulator or a device.

## Checks

Logs are in `clients/checks/`. These results are not a simulator launch, a device launch, or a deployment.

| Check | Result | Log |
| --- | --- | --- |
| OpenAPI export, server not started | Passed. Missing, extra and empty arguments exit 2. A missing parent directory is created. | `openapi-export.log`, `openapi-args.log` |
| Web ESLint | Passed after the PropsWithChildren and config-key fixes | `web-lint.log` |
| Web `tsc --noEmit` | Passed | `web-typecheck.log` |
| Web `next build` | Passed. `/` is a static page. The site is not deployed. | `web-build.log` |
| iOS Simulator SDK | Passed with Xcode 27.2. Built `MinimumOSVersion` is 27.0. Signing was off. The simulator was not launched. | `ios-build.log` |
| Android `assembleDebug` | The first run failed while downloading Maven artifacts (`android-build.log`, TLS handshake to `dl.google.com`). The retry passed and wrote `app-debug.apk`. Nothing was installed on an emulator or a device. | `android-build-2.log` |

The Android SDK used for that APK is `platforms;android-37.0` (`Platform.Version` 17, `AndroidVersion.ApiLevel` 37.0), Build-Tools 36.0.0, and Platform-Tools 37.0.1. AGP installed Platform-Tools during the build. `local.properties` is not committed.
