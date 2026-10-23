import { test } from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";
import { runInNewContext } from "node:vm";

const bundle = (
  await build({
    entryPoints: ["src/theme/function-keys.ts"],
    bundle: true,
    write: false,
    format: "iife",
  })
).outputFiles[0].text;

function page() {
  const host = new EventTarget();
  runInNewContext(bundle, { window: host, EventTarget });
  const fire = (type, props, target = host) => {
    const event = new Event(type, { cancelable: true, bubbles: true });
    Object.assign(event, props);
    let reachedPage = false;
    host.addEventListener(type, () => (reachedPage = true), { once: true });
    target.dispatchEvent(event);
    return { prevented: event.defaultPrevented, reachedPage };
  };
  return { host, fire };
}

test("Right Arrow's U+F703 is blocked before it can be inserted", () => {
  const { fire } = page();
  assert.equal(fire("beforeinput", { data: "\uF703" }).prevented, true);
  const keypress = fire("keypress", { charCode: 0xf703, key: "\uF703" });
  assert.equal(keypress.prevented, true);
  assert.equal(
    keypress.reachedPage,
    false,
    "editor keypress handlers never see it",
  );
  for (const code of [0xf700, 0xf704, 0xf729, 0xf74f])
    assert.equal(
      fire("beforeinput", { data: String.fromCharCode(code) }).prevented,
      true,
      code.toString(16),
    );
});

test("ordinary typing, navigation and the Apple logo are untouched", () => {
  const { fire } = page();
  for (const data of ["a", "/", "é", "😀", "\uF8FF", "\uF6FF", "\uF750", null])
    assert.equal(fire("beforeinput", { data }).prevented, false, String(data));
  assert.equal(fire("keypress", { charCode: 97, key: "a" }).prevented, false);
  assert.equal(fire("keydown", { key: "ArrowRight" }).prevented, false);
});

test("a function key that still lands in a text field is removed in place", () => {
  const field = Object.assign(new EventTarget(), {
    value: "ab\uF703\uF703c",
    selectionStart: 4,
    selectionEnd: 4,
    setSelectionRange(start, end) {
      this.selectionStart = start;
      this.selectionEnd = end;
    },
  });
  // The guard listens at the window; installing it on the field itself gives the
  // listener the same event target a real textarea would.
  runInNewContext(bundle, { window: field, EventTarget });
  field.dispatchEvent(new Event("input", { bubbles: true }));
  assert.deepEqual(Array.from(field.value), ["a", "b", "c"]);
  assert.equal(field.selectionStart, 2, "caret stays after the kept text");
  // Clean text is left alone, caret included.
  field.value = "abc";
  field.setSelectionRange(1, 1);
  field.dispatchEvent(new Event("input", { bubbles: true }));
  assert.equal(field.value, "abc");
  assert.equal(field.selectionStart, 1);
});
