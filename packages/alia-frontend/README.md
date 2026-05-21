# alia-frontend

Cross-platform consumer app for the Alia personal AI assistant. Built with React Native and Expo, communicates with `alia-server` via REST and WebSocket.

## Prerequisites

- Node.js (managed via mise from workspace root)
- pnpm

## Getting Started

```bash
# Install dependencies
pnpm install

# Start Expo dev server
pnpm dev
```

Open the app in development:

- **iOS**: press `i` to launch in the iOS simulator _(Mac only)_
- **Android**: press `a` to launch in the Android emulator
- **Web**: press `w` to run in a browser

Native device testing must use a self-built development app or production
build, not Expo Go. Alia ships a local Expo native module for mDNS and BLE, and
that code is outside Expo Go's runtime surface.

## Project Structure

```
alia-frontend/
├── app/                # Expo Router pages
│   ├── _layout.tsx     # Root layout
│   ├── index.tsx       # Home screen
│   └── +not-found.tsx  # 404 page
├── assets/             # Static assets (images, fonts)
├── components/
│   └── ui/             # Reusable UI components (React Native Reusables)
├── lib/                # Utilities, API clients, discovery adapter
├── modules/            # Local Expo native modules
├── app.json            # Expo configuration
├── metro.config.js     # Metro bundler config
└── tsconfig.json       # TypeScript config
```

## Tech Stack

- [Expo](https://expo.dev/) + [Expo Router](https://expo.dev/router)
- [React Native](https://reactnative.dev/)
- [Uniwind](https://uniwind.dev/) (Tailwind CSS for React Native)
- [React Native Reusables](https://reactnativereusables.com) (UI component library)
- [SWR](https://swr.vercel.app/) for REST request state and mutations
- [FlashList](https://shopify.github.io/flash-list/) for virtualized resource lists
- TypeScript

## Discovery Model

Web builds default to the same-origin backend so the frontend can be served by
`alia-server` without extra configuration. Native mobile builds keep a manual
server URL fallback and include first-run discovery in the self-built app:

- browse `_alia-server._tcp` over mDNS and present LAN servers before login;
- scan the embedded BLE provisioning service
  `3d4d5f9a-d0b9-4a7f-9f47-9d6cc7b5b4f1`;
- write Wi-Fi credentials to embedded BLE characteristic
  `3d4d5f9a-d0b9-4a7f-9f47-9d6cc7b5b4f3` and poll status from
  `3d4d5f9a-d0b9-4a7f-9f47-9d6cc7b5b4f2`;
- keep `_alia-server._tcp` and `_alia-embedded._tcp` visually separate so users
  bind edge nodes to a chosen server instead of mistaking devices for servers.

Once connected, the app polls server device APIs and prompts when the embedded
channel discovers an unbound `alia-embedded` candidate. Server discovery uses
`_alia-server._tcp`; embedded discovery uses `_alia-embedded._tcp` plus the
embedded BLE provisioning beacon.

The native implementation lives in `modules/alia-discovery` and is exposed to
the app through `lib/discovery`. The web adapter returns empty/unsupported
results so `expo export --platform web` stays free of native-only dependencies.
Android requests the required BLE runtime permissions before scanning or
connecting. iOS declares Bluetooth, local network, and Bonjour usage in
`app.json`.

### Native Build Notes

Use EAS or a local native build for phone testing:

```bash
pnpm --filter alia-frontend exec expo prebuild
pnpm --filter alia-frontend exec expo run:ios
pnpm --filter alia-frontend exec expo run:android
```

When adding native discovery libraries, keep their JS API behind a small
`lib/discovery` adapter so web builds, server-hosted frontend, and future
non-Expo clients can share the same UI without depending on mobile-only
modules.

Useful checks:

```bash
pnpm --filter alia-frontend exec tsc --noEmit
pnpm --filter alia-frontend exec expo export --platform web
```

## Adding UI Components

```bash
npx @react-native-reusables/cli@latest add [...components]
```

## Deploy

Deploy via [EAS](https://expo.dev/eas):

```bash
eas build    # Build for iOS/Android
eas update   # OTA updates
eas submit   # Submit to App Store / Google Play
```
