const QUICKCONNECT_ID = /^[a-z0-9][a-z0-9-]{0,62}$/i;
const QUICKCONNECT_SUBDOMAIN =
  /^([a-z0-9][a-z0-9-]{0,62})\.(quickconnect\.(?:to|cn))$/i;

// Synology QuickConnect input -> the canonical stored link
// https://quickconnect.to/<id>/<path>/ (path defaults to /chat/). Accepts a bare
// ID, the shareable quickconnect.to/<id> link and <id>.quickconnect.to. The
// native side resolves it to a verified NAS address; null means "not QuickConnect".
export function quickConnectUrl(input: string): string | null {
  const value = input.trim();
  if (!value || /[\\\s?#@]/.test(value)) return null;
  if (QUICKCONNECT_ID.test(value))
    return `https://quickconnect.to/${value.toLowerCase()}/chat/`;
  let url: URL;
  try {
    url = new URL(/^https?:\/\//i.test(value) ? value : `https://${value}`);
  } catch {
    return null;
  }
  if (url.port || url.username || url.password) return null;
  const host = url.hostname.toLowerCase();
  const segments = url.pathname.split("/").filter(Boolean);
  let domain: string;
  let id: string | undefined;
  if (host === "quickconnect.to" || host === "quickconnect.cn") {
    domain = host;
    id = segments.shift();
  } else {
    const match = QUICKCONNECT_SUBDOMAIN.exec(host);
    if (!match) return null;
    id = match[1];
    domain = match[2].toLowerCase();
  }
  if (!id || !QUICKCONNECT_ID.test(id)) return null;
  const path = segments.length ? `/${segments.join("/")}/` : "/chat/";
  return `https://${domain}/${id.toLowerCase()}${path}`;
}

export function normalizeServer(input: string): string {
  const value = input.trim();
  if (!/^https?:\/\//i.test(value) || /[\\\s]/.test(value))
    throw new Error("Enter a valid http:// or https:// server URL.");
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new Error("Enter a valid server URL.");
  }
  if (!url.hostname || url.username || url.password || url.search || url.hash)
    throw new Error(
      "Use a server URL without credentials, query parameters or fragments.",
    );
  url.pathname = url.pathname.replace(/\/+$/, "") + "/";
  return url.href;
}
