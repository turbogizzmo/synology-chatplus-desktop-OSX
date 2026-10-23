# GIF tab (proof of concept)

Adds a **GIF** tab next to **Emoji** and **Sticker** in the Synology ChatPlus
composer's emoji panel. ChatPlus has no GIF search of its own. The tab shows a
"Search KLIPY" box and a looping preview grid (trending first). Clicking a GIF
attaches it to the message as a file; it does **not** auto-send. You review it
and press send yourself.

Only ChatPlus gets the tab. Classic Synology Chat is a different web client and
isn't covered.

## How it fits into ChatPlus

The structure comes from the ChatPlus web bundle (`synochat-app.js`, build
1.0.0-00776), which the NAS serves publicly at
`/webman/3rdparty/ChatPlus/synochat-app.js`:

```
div[data-testid="emoji-sticker-picker"]   360x460 flex column, in a Tippy popover on <body>
├─ div.v-segment-control                   the Emoji | Sticker tab strip
│   ├─ div.v-segment-item[.v-segment-item--active] > div.v-segment-item__content
│   └─ div.v-segment-item                           > div.v-segment-item__content
└─ emoji-mart picker, or the sticker grid
```

- The GIF tab is a `div.v-segment-item` added to that strip, so it uses
  Synology's own tab styling. Selecting it hides the emoji/sticker content and
  shows the GIF pane; selecting Emoji or Sticker switches back.
- Vue owns the Emoji/Sticker selection state, so the GIF tab only moves the
  `v-segment-item--active` class visually and restores it when you leave.
- Only the composer's picker has the tab strip, so reaction pickers (same
  component, no strip) are left alone.
- The picker opens in a Tippy popover. After a GIF is attached, the panel closes
  through a simulated outside click, which is the same thing that happens when
  you click elsewhere.

## How a GIF is delivered

The Synology **Android** app does this through the keyboard. Decompiling
`Android-SynologyChatPlus.1.0.0-307.ww.apk` showed no GIF backend of Synology's
own (no `giphy`/`tenor`/`klipy` references). The keyboard (Gboard etc.) owns GIF
search, and Synology accepts the committed `image/gif` through Android
rich-content insertion, then **uploads it as a file** (OkHttp multipart to
`SYNO.ChatPlus.UploadedFile`).

The web client has a matching entry point. The message box has a paste target,
`[data-testid="message-create-box-paste-zone"]`, that reads files from
`clipboardData.items` and passes them to ChatPlus's own `validateAndAddFiles`. The
GIF tab downloads the GIF and dispatches a synthetic `paste` carrying it as a
`File` on that zone. Synology's authenticated client then stages the attachment
itself, so its upload API, session and `synotoken` are never touched.

Drag-and-drop was the first attempt. The source shows ChatPlus only accepts drops
on a `drop-zone` overlay it renders _while a real file drag is over the page_, so
a synthetic drop has nowhere to land. Paste avoids that.

The GIF goes to the paste zone you last clicked or focused (the one around the
emoji button you used), falling back to the main composer.

### Direct WebAPI upload (documented fallback, not used)

- Endpoint: `POST /webapi/entry.cgi`
- Upload: `api=SYNO.ChatPlus.UploadedFile`, `method=upload` (method name
  inferred), `multipart/form-data` with the file. Returns a file id.
- Send: `api=SYNO.ChatPlus.Post` with `file_ids` and `channel_id`.
- Needs the session cookie and a `synotoken` (CSRF), which is why the paste path
  is preferred.

## DSM's Content-Security-Policy

DSM serves ChatPlus with a CSP whose `img-src` only allows Synology/Google hosts
plus `data:` and `blob:`, while `connect-src` allows any `https:` host. So:

- `<img src="https://static.klipy.com/...">` is **blocked**. That's why the first
  version showed titles but no images.
- `fetch()` from KLIPY is **allowed**, and KLIPY's media sends
  `Access-Control-Allow-Origin: *`. Previews are downloaded and shown as `blob:`
  URLs, and the attached GIF is downloaded the same way.
- A pasted KLIPY _link_ couldn't render as an image on this page either, which
  is another reason to attach a file.

## Why KLIPY (not Tenor or GIPHY)

- **Tenor**: Google shut down the API on 2026-06-30 (new keys blocked since
  2026-01-13).
- **GIPHY**: free beta keys are capped at 100 calls/hour, and the unlimited
  production key now costs money. One key shared by every user would hit the cap
  almost immediately.
- **KLIPY**: free, including unlimited production access (requested through a
  form in the KLIPY partner panel). It's what Discourse, Zulip, Ghost, Odoo and
  Status moved to after Tenor shut down.

The tab uses KLIPY's Tenor-compatible v2 API (`GET https://api.klipy.com/v2/search`
and `/v2/featured`, with `key`, `q`, `limit`, `media_filter`, `contentfilter`,
`locale`). It shows `tinygif` in the grid and attaches `mediumgif`, the same
choice Zulip makes. The backend sits in `gifSource` in
[`src/theme/gif-picker.ts`](../src/theme/gif-picker.ts) so another provider can be
swapped in without touching the UI.

**Attribution:** KLIPY requires **"Search KLIPY"** as the search placeholder,
which the tab uses. "Powered by KLIPY" is optional; the tab shows it in a footer.

## One key for every user

Like the keyboard on Android, the app carries one developer key, so no user ever
needs their own. This follows [gifs-for-github](https://github.com/N1ck/gifs-for-github),
a browser extension that also injects a GIF picker into someone else's site:

- [`scripts/build-theme.mjs`](../scripts/build-theme.mjs) replaces `KLIPY_API_KEY`
  at build time using esbuild `define`, reading `process.env.KLIPY_API_KEY`.
- [`.github/workflows/release.yml`](../.github/workflows/release.yml) passes the
  `KLIPY_API_KEY` repository secret to the release build step only, the same way
  `TAURI_SIGNING_PRIVATE_KEY` is handled.
- The key ends up in the shipped bundle but is never committed. That's fine:
  KLIPY keys are client keys meant to ship inside apps.

**One-time setup:** create a KLIPY account at partner.klipy.com, generate an API
key, request production access, and add the key as a repository secret named
`KLIPY_API_KEY` (GitHub → Settings → Secrets and variables → Actions).

## How it's wired in

1. [`src/theme/gif-picker.ts`](../src/theme/gif-picker.ts): the GIF tab (IIFE;
   the pane uses a Shadow DOM so ChatPlus CSS can't reach it).
2. [`scripts/build-theme.mjs`](../scripts/build-theme.mjs) bundles it to
   `src-tauri/gif-picker.js` (git-ignored, generated by `npm run build`) and bakes
   in the KLIPY key.
3. [`src-tauri/src/services.rs`](../src-tauri/src/services.rs) injects it as an
   initialization script for `SynologyChatplus` only.

## Running it

```sh
KLIPY_API_KEY="your_klipy_key" npm run tauri dev
```

The key has to be set when the bundle is **built** (`tauri dev` rebuilds it
first), not just when the app launches. Without it the tab still opens but shows
"No KLIPY API key was built in."

Notes:

- Quit any installed ChatPlus Desktop first. The app allows one instance, so a
  dev build launched while it's running just hands off to it and exits.
- `tauri dev` doesn't notice changes to `src-tauri/gif-picker.js` because the
  file is git-ignored. Restart `tauri dev` after changing the picker.

## What still needs live verification

- **Paste acceptance**: confirm the synthetic `paste` stages the GIF as an
  attachment. If WebKit drops `clipboardData` from the constructed event, the
  code patches it back on; check that ChatPlus still sees the file.
- **Panel close**: confirm the simulated outside click closes the Tippy popover
  after a GIF is attached.
- **Thread replies**: confirm a GIF picked from a thread composer's emoji button
  lands in that thread's message box.
- **Tab state**: switch GIF → Emoji → GIF → Sticker and confirm only one tab
  looks selected each time.
