import { test } from "node:test";
import assert from "node:assert/strict";
import { transform } from "esbuild";
import { readFile } from "node:fs/promises";
const { code } = await transform(
  await readFile("src/app/navigation.ts", "utf8"),
  { loader: "ts", format: "esm" },
);
const { normalizeServer, quickConnectUrl } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`
);
test("normalizes server URLs", () => {
  assert.equal(
    normalizeServer(" https://nas.example.com/chat "),
    "https://nas.example.com/chat/",
  );
  assert.equal(
    normalizeServer("http://localhost:5000/chat///"),
    "http://localhost:5000/chat/",
  );
});
test("rejects credentials, tokens, non-web schemes and malformed input", () => {
  for (const value of [
    "file:///tmp",
    "javascript:alert(1)",
    "https:example.com",
    "https://user:secret@example.com",
    "https://example.com/?token=x",
    "https://example.com/#x",
    "https://exa mple.com",
    "https://example.com\\evil",
    "",
  ])
    assert.throws(() => normalizeServer(value));
});

test("QuickConnect IDs and links become the canonical stored link", () => {
  for (const [input, expected] of [
    ["MyNas", "https://quickconnect.to/mynas/chat/"],
    [" mynas ", "https://quickconnect.to/mynas/chat/"],
    ["quickconnect.to/mynas", "https://quickconnect.to/mynas/chat/"],
    ["https://QuickConnect.to/mynas/", "https://quickconnect.to/mynas/chat/"],
    ["http://quickconnect.to/mynas", "https://quickconnect.to/mynas/chat/"],
    [
      "https://quickconnect.to/mynas/chatplus",
      "https://quickconnect.to/mynas/chatplus/",
    ],
    ["mynas.quickconnect.to", "https://quickconnect.to/mynas/chat/"],
    ["https://mynas.quickconnect.cn/", "https://quickconnect.cn/mynas/chat/"],
  ])
    assert.equal(quickConnectUrl(input), expected, input);
  // Canonical output round-trips through the ordinary URL normalizer unchanged.
  assert.equal(
    normalizeServer(quickConnectUrl("mynas")),
    "https://quickconnect.to/mynas/chat/",
  );
});

test("ordinary server URLs and malformed input are not QuickConnect", () => {
  for (const input of [
    "",
    "https://nas.example.com/chat/",
    "https://mynas.direct.quickconnect.to/chat/",
    "https://quickconnect.to/",
    "https://quickconnect.to:8443/mynas",
    "https://user@quickconnect.to/mynas",
    "https://quickconnect.to/mynas?x=1",
    "-bad",
    "my_nas",
    "my nas",
    "192.168.1.20",
  ])
    assert.equal(quickConnectUrl(input), null, input);
});
