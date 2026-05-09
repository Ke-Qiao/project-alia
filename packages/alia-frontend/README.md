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

Open the app in:

- **iOS**: press `i` to launch in the iOS simulator _(Mac only)_
- **Android**: press `a` to launch in the Android emulator
- **Web**: press `w` to run in a browser

You can also scan the QR code using the [Expo Go](https://expo.dev/go) app on your device.

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
├── lib/                # Utilities (theme, helpers)
├── app.json            # Expo configuration
├── metro.config.js     # Metro bundler config
└── tsconfig.json       # TypeScript config
```

## Tech Stack

- [Expo](https://expo.dev/) + [Expo Router](https://expo.dev/router)
- [React Native](https://reactnative.dev/)
- [Uniwind](https://uniwind.dev/) (Tailwind CSS for React Native)
- [React Native Reusables](https://reactnativereusables.com) (UI component library)
- TypeScript

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
