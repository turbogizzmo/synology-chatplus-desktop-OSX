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
