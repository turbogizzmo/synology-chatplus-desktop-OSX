import { test } from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";
import { parseHTML } from "linkedom";
import { runInNewContext } from "node:vm";
import { bridgeMessages } from "./helpers/bridge-messages.mjs";

const bundle = async (entry) =>
  (
    await build({
      entryPoints: [entry],
      bundle: true,
      write: false,
      format: "iife",
    })
  ).outputFiles[0].text;
const unread = await bundle("src/theme/unread.ts");
const shim = await bundle("src/theme/webkit-notifications.ts");

test("macOS: unread bootstrap posts through the WKScriptMessageHandler bridge", () => {
  const { window: dom, document } = parseHTML(`<html><body class="eos-scope">
    <div><div id="sidebar-tab-item-messages"><span data-testid="tab-item-indicator"></span></div></div>
  </body></html>`);
  const bridge = bridgeMessages();
  const tasks = [];
  const host = {
    __chatplusProvider: "synology-chatplus",
    location: { href: "https://example.com/", pathname: "/", hash: "" },
    MutationObserver: dom.MutationObserver,
    webkit: {
      messageHandlers: { chatplusBridge: { postMessage: bridge.postMessage } },
    },
    addEventListener() {},
    removeEventListener() {},
    dispatchEvent: () => true,
  };
  host.top = host;
  document.hasFocus = () => true;
  Object.defineProperty(document, "visibilityState", { value: "visible" });
  Object.defineProperty(document, "defaultView", { value: host });
  runInNewContext(unread, {
    window: host,
    document,
    Event: dom.Event,
    KeyboardEvent: class {},
    queueMicrotask: (callback) => tasks.push(callback),
  });
  while (tasks.length) tasks.shift()();
  assert.equal(bridge.messages.length, 1);
  assert.equal(bridge.messages[0].chatplusUnread, 1);
  assert.equal(bridge.messages[0].source, "chatplus-dom");
});

function page({ frame = false, handler = true } = {}) {
  const posted = [];
  const tasks = [];
  const host = {
    crypto: { getRandomValues: (bytes) => bytes.fill(7) },
    ...(handler
      ? {
          webkit: {
            messageHandlers: {
              chatplusBridge: {
                postMessage: (value) => posted.push(JSON.parse(value)),
              },
            },
          },
        }
      : {}),
  };
  host.top = frame ? {} : host;
  runInNewContext(shim, {
    window: host,
    EventTarget,
    Event,
    TypeError,
    queueMicrotask: (callback) => tasks.push(callback),
  });
  const flush = () => {
    while (tasks.length) tasks.shift()();
  };
  return { host, posted, flush };
}

test("macOS shim replaces Notification and reports permission as granted", async () => {
  const { host } = page();
  assert.equal(typeof host.Notification, "function");
  assert.equal(host.Notification.permission, "granted");
  let callback;
  assert.equal(
    await host.Notification.requestPermission((value) => (callback = value)),
    "granted",
  );
  assert.equal(callback, "granted");
});

test("macOS shim forwards bounded notification content to the native bridge", () => {
  const { host, posted, flush } = page();
  const shown = [];
  const first = new host.Notification("Alice", {
    body: "x".repeat(5000),
    tag: "room-1",
    silent: true,
  });
  first.onshow = () => shown.push("first");
  const second = new host.Notification("Bob");
  second.addEventListener("show", () => shown.push("second"));
  flush();
  assert.deepEqual(shown, ["first", "second"]);
  assert.equal(posted.length, 2);
  assert.equal(posted[0].chatplusNotification, 1);
  assert.equal(posted[0].title, "Alice");
  assert.equal(posted[0].body.length, 4096);
  assert.equal(posted[0].tag, "room-1");
  assert.equal(posted[0].silent, true);
  assert.equal(posted[1].silent, false);
  assert.equal(posted[1].body, "");
  // Ids distinguish events within one page load; the session separates reloads.
  assert.notEqual(posted[0].id, posted[1].id);
  assert.equal(posted[0].session, posted[1].session);
  assert.match(posted[0].session, /^[0-9a-f]{32}$/);
});

test("macOS shim close fires once and a missing bridge reports an error", () => {
  const { host, flush } = page();
  const closed = [];
  const notification = new host.Notification("Alice");
  notification.onclose = () => closed.push(1);
  notification.close();
  notification.close();
  flush();
  assert.equal(closed.length, 1);
  assert.throws(() => new host.Notification(), TypeError);

  const detached = page({ handler: false });
  const errors = [];
  const orphan = new detached.host.Notification("Alice");
  orphan.onerror = () => errors.push(1);
  detached.flush();
  assert.equal(errors.length, 1);
});

test("macOS shim is not installed into subframes", () => {
  const { host } = page({ frame: true });
  assert.equal(host.Notification, undefined);
});
