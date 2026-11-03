/**
 * Pull download URLs out of arbitrary pasted or dropped text.
 *
 * Sources are messier than a single field implies: `text/uri-list` is
 * newline-separated with `#` comment lines, a copied block from a page arrives
 * with surrounding prose, and a saved link list can be separated by newlines,
 * commas or spaces. Splitting on whitespace and keeping only what parses as an
 * http(s) URL handles all of them without needing to know which one this is.
 *
 * Duplicates are dropped in place: pasting the same list twice should not queue
 * everything twice.
 */
export function parseUrls(text: string): string[] {
  const seen = new Set<string>();
  const urls: string[] = [];

  for (const raw of text.split(/[\s,]+/)) {
    // `text/uri-list` comment lines, and Markdown-ish wrappers around a link.
    const candidate = raw.trim().replace(/^[<("']+|[>)"',.]+$/g, "");
    if (!candidate || candidate.startsWith("#")) continue;

    let parsed: URL;
    try {
      parsed = new URL(candidate);
    } catch {
      continue;
    }
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") continue;

    if (seen.has(parsed.href)) continue;
    seen.add(parsed.href);
    // Pass the canonical form onward: the engine's protocol guard is
    // intentionally strict, while URL accepts harmless casing variations.
    urls.push(parsed.href);
  }

  return urls;
}

/**
 * Browser image/link drags often carry only an HTML fragment. Pull the real
 * targets out of that fragment, then send them through the same protocol and
 * deduplication checks as pasted text.
 */
export function parseHtmlUrls(html: string): string[] {
  if (!html.trim()) return [];

  if (typeof DOMParser !== "undefined") {
    const document = new DOMParser().parseFromString(html, "text/html");
    const media = Array.from(
      document.querySelectorAll<HTMLImageElement | HTMLMediaElement>(
        "img[src], video[src], audio[src], source[src]",
      ),
      (element) => element.getAttribute("src") ?? "",
    );
    if (media.length > 0) return parseUrls(media.join("\n"));
    return parseUrls(
      Array.from(document.querySelectorAll<HTMLAnchorElement>("a[href]"), (element) =>
        element.getAttribute("href") ?? "",
      ).join("\n"),
    );
  }

  // Node's test runner has no DOMParser. This fallback also covers older
  // webviews without turning an HTML fragment into executable DOM.
  const media = Array.from(
    html.matchAll(/<(?:img|video|audio|source)\b[^>]*\bsrc\s*=\s*(["'])(.*?)\1/gi),
    (match) => match[2] ?? "",
  );
  if (media.length > 0) return parseUrls(media.join("\n"));
  return parseUrls(
    Array.from(html.matchAll(/<a\b[^>]*\bhref\s*=\s*(["'])(.*?)\1/gi), (match) => match[2] ?? "").join("\n"),
  );
}

/**
 * URLs from a drop, whether the payload was text or a file of links.
 *
 * A dropped `.txt`/`.csv` is read; anything else is ignored rather than
 * guessed at — dropping a video file onto a download manager is not a request
 * to download it.
 */
export async function urlsFromDataTransfer(data: DataTransfer): Promise<string[]> {
  const fromHtml = parseHtmlUrls(data.getData("text/html"));
  if (fromHtml.length > 0) return fromHtml;

  const fromText = parseUrls(
    [data.getData("text/uri-list"), data.getData("text/plain")].filter(Boolean).join("\n"),
  );
  if (fromText.length > 0) return fromText;

  const readable = Array.from(data.files).filter(
    (file) => file.type.startsWith("text/") || /\.(txt|csv|uri|urls)$/i.test(file.name),
  );
  const contents = await Promise.all(readable.map((file) => file.text().catch(() => "")));
  return parseUrls(contents.join("\n"));
}
