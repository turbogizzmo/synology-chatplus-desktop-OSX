# ChatPlus Desktop for macOS

<img src="public/chatplus.png" alt="ChatPlus Desktop app icon" width="64" />

An unofficial, open-source **macOS** desktop client for Synology ChatPlus and
Synology Chat, with multi-service messaging support. It brings your self-hosted
chat on a Synology NAS and services such as Discord into one lightweight Tauri app.

[![CI](https://github.com/turbogizzmo/synology-chatplus-desktop-OSX/actions/workflows/ci.yml/badge.svg)](https://github.com/turbogizzmo/synology-chatplus-desktop-OSX/actions/workflows/ci.yml)
[![macOS](https://img.shields.io/badge/macOS-11%2B-000000)](#build-from-source)
[![GPLv3](https://img.shields.io/badge/license-GPLv3-blue)](LICENSE)
[![Tauri 2](https://img.shields.io/badge/Tauri-2-24C8DB)](https://v2.tauri.app/)

## Supported services

| Service           | Status                                                        |
| ----------------- | ------------------------------------------------------------- |
| Synology ChatPlus | Primary; dedicated theme, unread and notification integration |
| Synology Chat     | Unread and notification integration                           |
| Discord           | Unread and notification integration                           |
| Mattermost        | Experimental web session; no desktop unread/notifications     |
| Slack             | Experimental web session; no desktop unread/notifications     |

Provider login/SSO, calls and other features depend on each web app and its
policies. See [provider details](docs/PROVIDERS.md).

## Features

- Multiple chat services in one window, with persistent per-service sessions.
- Per-service unread dots plus menu bar tray and Dock badge unread presentation.
- Native macOS notifications with Full, Sender/chat or Generic previews.
- Light, Dark and System themes; English, Polish, Spanish, German, French,
  Japanese and Traditional Chinese interface.
- Native menus, zoom/fullscreen, saved window geometry and optional start at login.

## Status

`0.7.1`. macOS is the only target of this repository. Builds are not notarized by
Apple, so macOS asks once before the first launch. Download the latest release from the
[download page](https://turbogizzmo.github.io/synology-chatplus-desktop-OSX/).

Installed apps update themselves: they check GitHub every few hours and install a new
version after you confirm a restart (Settings > Updates). Updates are signed with this
project's update key and rejected otherwise.

To publish a release, set the version in `package.json`, `package-lock.json`,
`src-tauri/Cargo.toml`, `src-tauri/Cargo.lock` and `src-tauri/tauri.conf.json`, then
push a matching tag (`v0.6.1`). The release workflow builds the universal app, signs
the update with the `TAURI_SIGNING_PRIVATE_KEY` repository secret and publishes it.

## Build from source

Requirements: macOS 11+, Xcode Command Line Tools, Rust stable (rustup), Node.js 22.12+.

```sh
npm ci
npm run tauri build
```

The app and disk image are written to `src-tauri/target/release/bundle/macos/` and
`src-tauri/target/release/bundle/dmg/`. Because the build is unsigned, open it the
first time with right-click > **Open**, or run:

```sh
xattr -dr com.apple.quarantine "src-tauri/target/release/bundle/macos/ChatPlus Desktop.app"
```

CI also uploads an unsigned `.dmg` as a workflow artifact on every run.

## Setup

1. Launch the app and enter your Synology ChatPlus/Synology Chat server URL, for
   example `https://nas.example.com/chat/`, then sign in through the provider's own page.
2. Use **Settings > Services** to add other services or separate accounts.
3. Enable Desktop notifications in Settings and allow ChatPlus Desktop in
   **System Settings > Notifications** when macOS asks.

If the app can't reach a service, it shows why instead of an empty window: the address
wasn't found, the server didn't answer or took too long, its certificate isn't trusted,
or the QuickConnect ID couldn't be resolved. Use **Edit connection** to fix the address
(check `https://` and the port, usually `5001`), or **Try again** once the server is
awake. A NAS with a self-signed certificate needs a trusted address such as its
QuickConnect ID or a domain with a valid certificate.

Settings **Save** keeps the window open with inline confirmation; **Close** is separate.

## Privacy

No advertising and no analytics. Providers load from their own servers; the app never
sees sign-in passwords. WKWebView keeps cookies and sessions locally, with a separate
profile for each added service. Diagnostics omit message content, credentials,
cookies/tokens and private URLs. See [SECURITY.md](SECURITY.md).

## Development

See [development](docs/DEVELOPMENT.md), [architecture](docs/ARCHITECTURE.md),
[notifications](docs/NOTIFICATIONS.md), [providers](docs/PROVIDERS.md),
[testing](docs/TESTING.md) and [CONTRIBUTING.md](CONTRIBUTING.md).

## Reporting bugs

[Open an issue](https://github.com/turbogizzmo/synology-chatplus-desktop-OSX/issues/new/choose)
with the app and macOS versions, provider version and safe reproduction steps.
About > **Copy diagnostics** supplies bounded, privacy-safe state. Never include
private messages, credentials/tokens, server/attachment URLs or profiles.

## License and credits

GNU GPLv3 (GPL-3.0-only). See [LICENSE](LICENSE), [NOTICE](NOTICE) and
[licensing notes](docs/LICENSING.md). This is a modified version of ChatPlus Desktop,
originally written by Swiph3l and distributed under the same license; the copyright
notices in NOTICE apply. Dependencies retain their own licenses.

Not affiliated with or endorsed by Synology or any other supported provider. Product
names and trademarks identify compatibility and belong to their respective owners.
