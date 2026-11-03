import type { ImageAlbum } from "./engine";

export function albumStatus(album: ImageAlbum): string {
  const found = album.items.length;
  if (album.expectedCount != null && album.expectedCount !== found) {
    return `Found ${found} of ${album.expectedCount}`;
  }
  if (album.completeness === "complete") return `Found ${found} media`;
  if (album.completeness === "partial") return `Found ${found} · incomplete`;
  return `Found ${found} · completeness unverified`;
}
