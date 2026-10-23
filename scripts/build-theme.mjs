import { build } from "esbuild";
import { readFile } from "node:fs/promises";
await build({
  entryPoints: ["src/theme/bootstrap.ts"],
  bundle: true,
  format: "iife",
  target: "es2022",
  outfile: "src-tauri/theme-bootstrap.js",
  plugins: [
    {
      name: "inline-css",
      setup(build) {
        build.onLoad({ filter: /\.css$/ }, async (args) => ({
          contents: await readFile(args.path.replace("?inline", ""), "utf8"),
          loader: "text",
        }));
      },
    },
  ],
});
// Swiph3l: Synology Chat and Discord need unread/focus adapters without ChatPlus CSS; keeping a separate bundle avoids changing remote provider styling.
await build({
  entryPoints: ["src/theme/unread.ts"],
  bundle: true,
  format: "iife",
  target: "es2022",
  outfile: "src-tauri/unread-bootstrap.js",
});
// macOS-only injection: WKWebView lacks the web Notification API that WebView2 bridges natively.
await build({
  entryPoints: ["src/theme/webkit-notifications.ts"],
  bundle: true,
  format: "iife",
  target: "es2022",
  outfile: "src-tauri/webkit-notifications.js",
});
// macOS-only injection for every provider: arrow keys move the caret instead of typing boxes.
await build({
  entryPoints: ["src/theme/arrow-keys.ts"],
  bundle: true,
  format: "iife",
  target: "es2022",
  outfile: "src-tauri/arrow-keys.js",
});
// GIF tab: injected only for ChatPlus, which has no native GIF search.
// The app-wide KLIPY key comes from the environment (a repository secret in the release
// workflow) so it ships in the bundle without being committed; unset builds show a notice.
await build({
  entryPoints: ["src/theme/gif-picker.ts"],
  bundle: true,
  format: "iife",
  target: "es2022",
  outfile: "src-tauri/gif-picker.js",
  define: {
    KLIPY_API_KEY: JSON.stringify(process.env.KLIPY_API_KEY ?? ""),
  },
});
