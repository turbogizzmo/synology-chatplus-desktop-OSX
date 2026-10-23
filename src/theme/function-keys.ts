// AppKit encodes arrow, F-key, Home/End and similar keys as characters in
// U+F700–U+F74F (NSUpArrowFunctionKey … NSModeSwitchFunctionKey). They are never
// text, but some editor paths in WKWebView insert them anyway, showing a box per
// key press (Right Arrow is U+F703). Block them on every insertion path; the
// Apple logo (U+F8FF) and all other characters are untouched.
const FUNCTION_KEYS = /[\uF700-\uF74F]/g;
const FIRST = 0xf700;
const LAST = 0xf74f;

export const isFunctionKeyText = (text: string | null | undefined) =>
  !!text && /[\uF700-\uF74F]/.test(text);

export const stripFunctionKeys = (text: string) =>
  text.replace(FUNCTION_KEYS, "");

interface TextField {
  value: string;
  selectionStart: number | null;
  selectionEnd: number | null;
  setSelectionRange(start: number, end: number): void;
}

function textField(target: EventTarget | null): TextField | null {
  const field = target as Partial<TextField> | null;
  return field &&
    typeof field.value === "string" &&
    typeof field.setSelectionRange === "function"
    ? (field as TextField)
    : null;
}

export function installFunctionKeyGuard(host: EventTarget) {
  // Native text insertion (and IME commits) announce themselves as beforeinput.
  host.addEventListener(
    "beforeinput",
    (event) => {
      if (isFunctionKeyText((event as InputEvent).data)) event.preventDefault();
    },
    true,
  );
  // Editors that insert from legacy keypress handlers never see these keys.
  host.addEventListener(
    "keypress",
    (event) => {
      const key = event as KeyboardEvent;
      if (
        (key.charCode >= FIRST && key.charCode <= LAST) ||
        isFunctionKeyText(key.key)
      ) {
        event.preventDefault();
        event.stopImmediatePropagation();
      }
    },
    true,
  );
  // Last resort for paths that bypass both events: clean the field in place
  // and keep the caret where the user expects it.
  host.addEventListener(
    "input",
    (event) => {
      const field = textField(event.target);
      if (!field || !isFunctionKeyText(field.value)) return;
      const caret = field.selectionStart ?? field.value.length;
      const before = stripFunctionKeys(field.value.slice(0, caret)).length;
      field.value = stripFunctionKeys(field.value);
      field.setSelectionRange(before, before);
    },
    true,
  );
}

(() => {
  if (typeof window === "undefined") return;
  installFunctionKeyGuard(window);
})();
