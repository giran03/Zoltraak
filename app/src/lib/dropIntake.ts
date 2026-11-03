import type { LinkInspection } from "./engine.ts";

/** The question asked before saving web pages instead of the files behind them. */
export function htmlConfirmMessage(pages: { url: string; filename: string }[]): string {
  const names = pages.map(page => `• ${page.filename}`).join("\n");
  return pages.length === 1
    ? `This link will download an HTML page, not a file:\n\n${names}\n\nDownload it anyway?`
    : `These ${pages.length} links will download HTML pages, not files:\n\n${names}\n\nDownload them anyway?`;
}

/**
 * Dropped links queue straight away; those that would save a web page wait
 * for a yes. A link that cannot be inspected is let through, since the
 * download itself reports anything that is wrong with it.
 */
export async function screenDroppedLinks(
  urls: string[],
  inspect: (url: string) => Promise<LinkInspection>,
  confirm: (message: string) => boolean,
): Promise<string[]> {
  const checked = await Promise.all(urls.map(async url => {
    try { return { url, inspection: await inspect(url) }; }
    catch { return { url, inspection: null }; }
  }));
  const pages = checked.flatMap(({ url, inspection }) => inspection?.isHtml ? [{ url, filename: inspection.filename }] : []);
  if (pages.length === 0 || confirm(htmlConfirmMessage(pages))) return urls;
  const declined = new Set(pages.map(page => page.url));
  return urls.filter(url => !declined.has(url));
}
