// Proof-of-concept GIF tab for the Synology ChatPlus emoji/sticker panel.
//
// This runs as an injected initialization script inside the provider WebView
// (see services.rs). ChatPlus has no GIF search, so this adds a "GIF" tab next
// to Emoji and Sticker in the composer's picker, with a search box and a
// looping-preview grid, like Discord or Slack.
//
// Structure (from the ChatPlus 1.0.0-00776 web bundle, synochat-app.js):
//   div[data-testid="emoji-sticker-picker"]   (360x460 flex column, in a Tippy
//   ├─ div.v-segment-control                   popover appended to <body>)
//   │   ├─ div.v-segment-item[--active]  > div.v-segment-item__content "Emoji"
//   │   └─ div.v-segment-item            > div.v-segment-item__content "Sticker"
//   └─ <emoji-mart picker> or <sticker grid>
// The tab strip only exists in the composer's picker, so reaction pickers are
// left alone. The GIF tab reuses Synology's own segment classes to look native.
//
// Provider note: Google shut down the Tenor API (June 2026) and GIPHY charges
// for unlimited production keys, so this uses KLIPY, which is free and is what
// Discourse, Zulip and Ghost moved to. KLIPY's v2 API is Tenor-compatible; the
// client below is isolated in `gifSource` so another backend can be swapped in
// without touching the UI. KLIPY requires "Search KLIPY" as the search
// placeholder. The app-wide key is injected at build time by esbuild `define`
// from the KLIPY_API_KEY environment variable (a repository secret in the
// release workflow), so it ships in the bundle but is never committed and no
// user ever needs their own key.
//
// Delivery mirrors the Synology Android app, where the keyboard commits an
// image/gif and the app uploads it as a file attachment. Here the GIF bytes are
// fetched and handed to the composer's paste zone
// ([data-testid="message-create-box-paste-zone"]) as a synthetic paste. ChatPlus
// listens there for pasted files and runs its own validateAndAddFiles, so the
// GIF is staged as an attachment by Synology's authenticated client; we never
// touch its upload API. It does NOT auto-send.
//
// DSM's Content-Security-Policy limits img-src to Synology/Google hosts plus
// data: and blob:, but allows connect-src to any https host, so previews are
// fetched and shown as blob: URLs (KLIPY media sends Access-Control-Allow-Origin
// *). Pointing <img> at static.klipy.com directly is blocked.

// Replaced at build time by scripts/build-theme.mjs; empty when unset.
declare const KLIPY_API_KEY: string;

declare global {
  interface Window {
    __chatplusGifPicker?: boolean;
  }
}

(() => {
  if (window.__chatplusGifPicker) return;
  window.__chatplusGifPicker = true;

  const PICKER = '[data-testid="emoji-sticker-picker"]';
  const SEGMENTS = ".v-segment-control";
  const PASTE_ZONE = '[data-testid="message-create-box-paste-zone"]';
  const THREAD = '[data-testid="thread-viewer-comments-section"]';
  const ACTIVE = "v-segment-item--active";
  const GIF_MODE = "data-chatplus-gif";
  // Marks the picker child that holds the tab strip; Vue leaves unknown
  // attributes alone, whereas it may rewrite className on re-render.
  const KEEP = "data-chatplus-gif-keep";
  const SEARCH_DEBOUNCE_MS = 350;
  const GRID_LIMIT = 24;

  // --- GIF backend (KLIPY, Tenor-compatible v2 API) --------------------------
  // Isolated so the UI below never references a specific provider's shapes.
  interface GifItem {
    id: string;
    previewUrl: string; // small looping preview for the grid
    sendUrl: string; // .gif uploaded as the attachment
    description: string;
  }

  const gifSource = {
    // Required by KLIPY's attribution guidelines.
    placeholder: "Search KLIPY",
    attribution: "Powered by KLIPY",
    key: KLIPY_API_KEY.trim(),
    async trending(): Promise<GifItem[]> {
      return this.fetch("https://api.klipy.com/v2/featured", {});
    },
    async search(query: string): Promise<GifItem[]> {
      return this.fetch("https://api.klipy.com/v2/search", { q: query });
    },
    async fetch(
      endpoint: string,
      params: Record<string, string>,
    ): Promise<GifItem[]> {
      const url = new URL(endpoint);
      url.searchParams.set("key", this.key);
      url.searchParams.set("limit", String(GRID_LIMIT));
      url.searchParams.set("contentfilter", "medium");
      // Only request the formats we use; KLIPY says this cuts responses ~70%.
      url.searchParams.set("media_filter", "tinygif,mediumgif,gif");
      url.searchParams.set("locale", navigator.language.replace("-", "_"));
      for (const [name, value] of Object.entries(params))
        url.searchParams.set(name, value);
      const response = await fetch(url.toString());
      if (!response.ok) throw new Error(`KLIPY ${response.status}`);
      const body = (await response.json()) as {
        results?: Array<{
          id: string;
          title?: string;
          content_description?: string;
          media_formats: Record<string, { url?: string }>;
        }>;
      };
      return (body.results ?? []).flatMap((item) => {
        const formats = item.media_formats;
        const preview = formats.tinygif?.url || formats.mediumgif?.url;
        // mediumgif keeps uploads small enough for chat, like Zulip does.
        const send = formats.mediumgif?.url || formats.gif?.url || preview;
        if (!preview || !send) return [];
        return [
          {
            id: item.id,
            previewUrl: preview,
            sendUrl: send,
            description: item.content_description || item.title || "GIF",
          },
        ];
      });
    },
  };

  // --- Which composer the picker belongs to ---------------------------------
  // The picker is appended to <body>, so remember the paste zone the user last
  // interacted with (the emoji button lives inside it).
  let lastPasteZone: HTMLElement | null = null;
  const remember = (event: Event) => {
    const target = event.target as Element | null;
    const zone = target?.closest?.<HTMLElement>(PASTE_ZONE);
    if (zone) lastPasteZone = zone;
  };
  document.addEventListener("pointerdown", remember, true);
  document.addEventListener("focusin", remember, true);

  function targetPasteZone(): HTMLElement | null {
    if (lastPasteZone?.isConnected) return lastPasteZone;
    const zones = [
      ...document.querySelectorAll<HTMLElement>(PASTE_ZONE),
    ].filter(
      (zone) => !zone.closest(THREAD) && zone.getClientRects().length > 0,
    );
    return zones[0] ?? null;
  }

  // Hand the composer a real GIF File the way a user paste would. ChatPlus reads
  // files from clipboardData.items in its paste-zone listener. Engines differ on
  // whether the ClipboardEvent constructor keeps clipboardData, so patch it on
  // when missing.
  async function attachGif(item: GifItem): Promise<string | null> {
    const zone = targetPasteZone();
    if (!zone) return "Open a chat first, then pick a GIF.";
    let file: File;
    try {
      const response = await fetch(item.sendUrl);
      if (!response.ok) throw new Error(`download ${response.status}`);
      const blob = await response.blob();
      file = new File([blob], `klipy-${item.id}.gif`, {
        type: blob.type || "image/gif",
      });
    } catch (error) {
      console.warn("[gif-picker] could not download GIF:", error);
      return "Couldn't download that GIF.";
    }
    const transfer = new DataTransfer();
    transfer.items.add(file);
    let event: Event;
    try {
      event = new ClipboardEvent("paste", {
        bubbles: true,
        cancelable: true,
        clipboardData: transfer,
      });
    } catch {
      event = new Event("paste", { bubbles: true, cancelable: true });
    }
    const data = (event as ClipboardEvent).clipboardData;
    if (!data || data.items.length === 0)
      Object.defineProperty(event, "clipboardData", { value: transfer });
    zone.dispatchEvent(event);
    return null;
  }

  // Tippy hides the picker on a press outside it (stickers call hide() after
  // sending; we can't reach that from here).
  function closePicker() {
    for (const type of ["mousedown", "mouseup", "click"])
      document.body.dispatchEvent(
        new MouseEvent(type, { bubbles: true, cancelable: true }),
      );
  }

  // --- Styles ----------------------------------------------------------------
  // While GIF mode is on, hide the picker's own content (emoji-mart / stickers)
  // and show our pane. DSM's CSP allows inline styles.
  const pageStyle = document.createElement("style");
  pageStyle.id = "chatplus-gif-picker-style";
  pageStyle.textContent = `
    ${PICKER} > .chatplus-gif-pane { display: none; }
    ${PICKER}[${GIF_MODE}] > :not([${KEEP}]):not(.chatplus-gif-pane) { display: none !important; }
    ${PICKER}[${GIF_MODE}] > .chatplus-gif-pane { display: flex; }
  `;

  // Grays that read on both light and dark surfaces; text color is inherited
  // from the picker so it follows Synology's theme.
  const paneStyle = `
    :host { flex: 1; min-height: 0; flex-direction: column; color: inherit; }
    * { box-sizing: border-box; font-family: inherit; }
    .search { padding: 0 12px 8px; }
    input {
      width: 100%; padding: 7px 10px; border-radius: 8px; font-size: 14px;
      border: 1px solid rgba(128,128,128,.35); background: rgba(128,128,128,.10);
      color: inherit; outline: none;
    }
    input:focus { border-color: #2e8fff; }
    .grid {
      flex: 1; min-height: 0; overflow-y: auto; padding: 0 12px 8px;
      display: grid; grid-template-columns: 1fr 1fr; gap: 6px; align-content: start;
    }
    .grid img {
      width: 100%; height: 104px; object-fit: cover; border-radius: 8px;
      cursor: pointer; background: rgba(128,128,128,.15);
    }
    .grid img:hover, .grid img:focus { outline: 2px solid #2e8fff; }
    .state { grid-column: 1 / -1; padding: 24px 8px; text-align: center; font-size: 13px; opacity: .7; }
    .footer { padding: 4px 12px 6px; font-size: 11px; opacity: .55; text-align: right; }
  `;

  // --- One GIF tab + pane per picker ----------------------------------------
  function mount(picker: HTMLElement, segments: HTMLElement) {
    let strip: HTMLElement = segments;
    while (strip.parentElement && strip.parentElement !== picker)
      strip = strip.parentElement;
    strip.setAttribute(KEEP, "");

    const tab = document.createElement("div");
    tab.className = "v-segment-item chatplus-gif-tab";
    tab.tabIndex = 0;
    tab.setAttribute("aria-disabled", "false");
    const label = document.createElement("div");
    label.className = "v-segment-item__content";
    label.textContent = "GIF";
    tab.appendChild(label);
    segments.appendChild(tab);

    const pane = document.createElement("div");
    pane.className = "chatplus-gif-pane";
    const shadow = pane.attachShadow({ mode: "open" });
    shadow.innerHTML = `
      <style>${paneStyle}</style>
      <div class="search"><input type="text" aria-label="Search GIFs" /></div>
      <div class="grid"></div>
      <div class="footer"></div>
    `;
    picker.appendChild(pane);

    const input = shadow.querySelector("input") as HTMLInputElement;
    const grid = shadow.querySelector(".grid") as HTMLElement;
    const footer = shadow.querySelector(".footer") as HTMLElement;
    input.placeholder = gifSource.placeholder;
    footer.textContent = gifSource.attribution;

    let requestToken = 0;
    let previewUrls: string[] = [];
    let loaded = false;

    const releasePreviews = () => {
      for (const url of previewUrls) URL.revokeObjectURL(url);
      previewUrls = [];
    };
    const setState = (message: string) => {
      releasePreviews();
      grid.innerHTML = "";
      const note = document.createElement("div");
      note.className = "state";
      note.textContent = message;
      grid.appendChild(note);
    };
    const showPreview = async (
      img: HTMLImageElement,
      item: GifItem,
      token: number,
    ) => {
      try {
        const response = await fetch(item.previewUrl);
        if (!response.ok) throw new Error(`preview ${response.status}`);
        const blob = await response.blob();
        if (token !== requestToken) return;
        const url = URL.createObjectURL(blob);
        previewUrls.push(url);
        img.src = url;
        // Set alt only once loaded so a pending tile doesn't render as text.
        img.alt = item.description;
      } catch (error) {
        console.warn("[gif-picker] preview failed:", error);
        img.remove();
      }
    };
    const choose = async (item: GifItem) => {
      const problem = await attachGif(item);
      if (problem) {
        setState(problem);
        return;
      }
      closePicker();
    };
    const render = (items: GifItem[], token: number) => {
      if (!items.length) {
        setState("No GIFs found.");
        return;
      }
      releasePreviews();
      grid.innerHTML = "";
      for (const item of items) {
        const img = document.createElement("img");
        img.alt = "";
        img.title = item.description;
        img.tabIndex = 0;
        img.addEventListener("click", () => void choose(item));
        img.addEventListener("keydown", (event) => {
          if (event.key === "Enter") void choose(item);
        });
        grid.appendChild(img);
        void showPreview(img, item, token);
      }
    };
    const load = async (query: string) => {
      if (!gifSource.key) {
        setState(
          "No KLIPY API key was built in. Set KLIPY_API_KEY and rebuild.",
        );
        return;
      }
      const token = ++requestToken;
      setState("Loading…");
      try {
        const items = query.trim()
          ? await gifSource.search(query.trim())
          : await gifSource.trending();
        if (token === requestToken) render(items, token);
      } catch (error) {
        if (token === requestToken)
          setState(`Could not load GIFs (${(error as Error).message}).`);
      }
    };

    let debounce: ReturnType<typeof setTimeout> | undefined;
    input.addEventListener("input", () => {
      clearTimeout(debounce);
      const value = input.value;
      debounce = setTimeout(() => void load(value), SEARCH_DEBOUNCE_MS);
    });
    // Keep typing in our box from reaching ChatPlus's own shortcuts.
    input.addEventListener("keydown", (event) => event.stopPropagation());

    // Vue owns the Emoji/Sticker active state. Entering GIF mode borrows the
    // native active class so only our tab looks selected; leaving restores it
    // when Vue won't re-render (clicking the tab that was already active).
    const enter = () => {
      if (picker.hasAttribute(GIF_MODE)) return;
      for (const item of segments.querySelectorAll(`.${ACTIVE}`))
        if (item !== tab) {
          item.classList.remove(ACTIVE);
          item.classList.add("chatplus-was-active");
        }
      tab.classList.add(ACTIVE);
      picker.setAttribute(GIF_MODE, "");
      if (!loaded) {
        loaded = true;
        void load("");
      }
      input.focus();
    };
    const leave = () => {
      if (!picker.hasAttribute(GIF_MODE)) return;
      picker.removeAttribute(GIF_MODE);
      tab.classList.remove(ACTIVE);
      for (const item of segments.querySelectorAll(".chatplus-was-active")) {
        item.classList.remove("chatplus-was-active");
        item.classList.add(ACTIVE);
      }
    };

    tab.addEventListener("click", enter);
    tab.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        enter();
      }
    });
    // Capture runs before the native item's own click/focus handlers.
    const nativeSelect = (event: Event) => {
      const item = (event.target as Element).closest(".v-segment-item");
      if (item && item !== tab && segments.contains(item)) leave();
    };
    segments.addEventListener("click", nativeSelect, true);
    segments.addEventListener("focusin", nativeSelect, true);
  }

  // Pickers are created lazily inside Tippy popovers; watch for new ones.
  let scheduled = false;
  const scan = () => {
    scheduled = false;
    const head = document.head ?? document.documentElement;
    if (!head) return; // Init scripts can run before the document exists.
    if (!pageStyle.isConnected) head.appendChild(pageStyle);
    for (const picker of document.querySelectorAll<HTMLElement>(PICKER)) {
      // Only the composer's picker has the Emoji/Sticker strip.
      const segments = picker.querySelector<HTMLElement>(SEGMENTS);
      if (!segments || segments.querySelector(".chatplus-gif-tab")) continue;
      mount(picker, segments);
    }
  };
  new MutationObserver(() => {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(scan);
  }).observe(document, { childList: true, subtree: true });
  scan();
})();

export {};
