//! Checking candidates against their own first bytes, before any of them
//! reach the download queue.
//!
//! Extraction produces URLs that *look* right. Whether they are right is a
//! different question, and one only the response can answer: a signed CDN link
//! whose token expired, a photo in a post that turned private between the
//! scrape and the click, a `lookaside` URL fetched without the crawler header —
//! all answer `200` with a page. Checking here rather than at download time
//! means the grid never offers a picture that is not there, and the filename is
//! settled before the queue picks one.
//!
//! The check costs one ranged request per candidate, run concurrently. A
//! request that fails outright proves nothing about the URL — the network may
//! simply be down — so it leaves the item unverified and downloadable, rather
//! than deleting something that would have worked.

use futures_util::stream::{self, StreamExt};
use reqwest::Client;

use super::sniff::{self, SNIFF_LEN};
use super::{ImageItem, MediaKind};
use crate::headers::{self, Headers};

/// How many candidates are checked at once.
///
/// A carousel holds at most a couple of dozen items and these are 32-byte
/// requests, so the ceiling is about being a good citizen towards one host
/// rather than about local resources.
const CONCURRENCY: usize = 6;

/// Verify every item in place.
///
/// Sets `verified` to `Some(true)` for a confirmed picture (correcting
/// `extension` when the bytes disagree with the URL), `Some(false)` for a
/// response that is definitely not one, and leaves it `None` when the check
/// could not be made.
pub async fn verify_all(client: &Client, items: &mut [ImageItem], base_headers: &Headers) {
    // Clips are exempt, and must be: this check passes only what has a picture
    // format's magic number, and an `ftyp` box is not one — so running it over
    // a video marked every clip in the post as "not an image" and deleted it.
    // Nothing about a video is knowable from 32 bytes anyway; the video
    // pipeline probes its own tracks and reports a bad source as a failed job.
    let targets: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.kind == MediaKind::Image)
        .map(|(index, _)| index)
        .collect();

    // The request inputs are gathered as owned values first. Mapping straight
    // off `items.iter()` would give each future a borrow of the slice, and a
    // closure returning a future that borrows its argument cannot satisfy the
    // `for<'a>` bound an async caller needs.
    let requests: Vec<(String, Headers)> = targets
        .iter()
        .map(|&index| {
            let item = &items[index];
            (item.url.clone(), merge_headers(base_headers, &item.headers))
        })
        .collect();

    let outcomes: Vec<Outcome> = stream::iter(requests)
        .map(|(url, headers)| {
            let client = client.clone();
            async move { verify_one(&client, &url, &headers).await }
        })
        .buffered(CONCURRENCY)
        .collect()
        .await;

    for (index, outcome) in targets.into_iter().zip(outcomes) {
        let item = &mut items[index];
        match outcome {
            Outcome::Image { extension, length } => {
                item.verified = Some(true);
                item.extension = extension;
                // Only fill a size we did not already know: a platform that
                // told us the byte count told us about the *original*, while
                // this is whatever rendition the URL resolves to today.
                if item.filesize.is_none() {
                    item.filesize = length;
                }
            }
            Outcome::NotImage => item.verified = Some(false),
            Outcome::Unknown => item.verified = None,
        }
    }
}

/// An item's own headers layered over the request-wide ones, with the item's
/// winning — it carries the identity that made extraction work for that
/// specific host (the Facebook crawler `User-Agent`, most often), and the
/// caller's browser `User-Agent` would otherwise get a login page back.
fn merge_headers(base: &Headers, item: &Headers) -> Headers {
    let mut merged = item.clone();
    for (name, value) in base {
        if !merged.iter().any(|(existing, _)| existing.eq_ignore_ascii_case(name)) {
            merged.push((name.clone(), value.clone()));
        }
    }
    merged
}

enum Outcome {
    Image { extension: String, length: Option<u64> },
    NotImage,
    Unknown,
}

/// The identity an image request needs when the caller supplied none.
///
/// Not cosmetic: plenty of hosts — Wikimedia among them — answer a request with
/// no `User-Agent` with a 403, so a check that omitted this condemned every
/// picture it looked at. `net::fetch_text` defaults the same way for the same
/// reason.
const DEFAULT_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

async fn verify_one(client: &Client, url: &str, headers: &Headers) -> Outcome {
    // A ranged request keeps this to the bytes actually needed. A host that
    // ignores `Range` answers `200` with the whole file, which is why the
    // response is read through a capped stream below rather than in full.
    let mut request = headers::apply(client.get(url), headers)
        .header("Range", format!("bytes=0-{}", SNIFF_LEN - 1))
        .header("Accept", "image/avif,image/webp,image/apng,image/*,*/*;q=0.8");
    if !headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("user-agent")) {
        request = request.header("User-Agent", DEFAULT_UA);
    }

    let Ok(response) = request.send().await else {
        return Outcome::Unknown;
    };

    let status = response.status();
    if !status.is_success() {
        // Only an answer that means "there is nothing here" condemns the item.
        // A 403, a 405, a 416 from a host that dislikes ranged requests, a 429
        // — those are verdicts on *this request*, not on the URL, and dropping
        // a picture over one would make the check worse than not having it.
        // The item stays, unverified, and the download decides.
        return if matches!(status.as_u16(), 404 | 410) {
            Outcome::NotImage
        } else {
            Outcome::Unknown
        };
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if sniff::is_definitely_not_image(&content_type) {
        return Outcome::NotImage;
    }

    // `Content-Range: bytes 0-31/12345` on a partial response names the whole
    // file; `Content-Length` only does when the server ignored the range.
    let length = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(crate::probe::total_from_content_range)
        .or_else(|| {
            (status != reqwest::StatusCode::PARTIAL_CONTENT)
                .then(|| response.content_length())
                .flatten()
        });

    let Some(head) = read_head(response).await else {
        return Outcome::Unknown;
    };

    match sniff::identify(&head) {
        Some(kind) => Outcome::Image { extension: kind.ext().to_string(), length },
        None => Outcome::NotImage,
    }
}

/// The first [`SNIFF_LEN`] bytes of a response, without buffering the rest.
///
/// A server that ignored the `Range` header is sending the entire file; taking
/// it whole to read 32 bytes would download every candidate in the album just
/// to look at it. The stream is abandoned as soon as there is enough.
async fn read_head(response: reqwest::Response) -> Option<Vec<u8>> {
    let mut head = Vec::with_capacity(SNIFF_LEN);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.ok()?;
        head.extend_from_slice(&chunk);
        if head.len() >= SNIFF_LEN {
            head.truncate(SNIFF_LEN);
            break;
        }
    }
    (!head.is_empty()).then_some(head)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_items_own_headers_outrank_the_request_wide_ones() {
        // The Facebook case: the crawler User-Agent is the only reason the
        // extraction worked, and the caller's browser one would get a login
        // page. The item's value has to survive the merge.
        let base = vec![
            ("User-Agent".to_string(), "Mozilla/5.0".to_string()),
            ("Cookie".to_string(), "session=abc".to_string()),
        ];
        let item = vec![("User-Agent".to_string(), "facebookexternalhit/1.1".to_string())];

        let merged = merge_headers(&base, &item);

        let ua = merged.iter().find(|(n, _)| n.eq_ignore_ascii_case("user-agent")).unwrap();
        assert_eq!(ua.1, "facebookexternalhit/1.1");
        // Everything the item did not speak to still comes through.
        assert!(merged.iter().any(|(n, v)| n == "Cookie" && v == "session=abc"));
        assert_eq!(merged.len(), 2);
    }

    #[tokio::test]
    async fn clips_are_never_judged_by_the_picture_test() {
        // The regression this guards: `verify_all` ran over every item, an mp4
        // header has no picture magic number, so every clip in a post came back
        // `verified: Some(false)` and was deleted by `analyze` before the UI
        // ever saw it. Videos must come out untouched — and with no request
        // spent on them, which a bogus host proves.
        let mut items = vec![
            ImageItem::video(
                "1",
                vec![crate::video::VideoFormat {
                    video_url: Some("https://nonexistent.invalid/clip.mp4".into()),
                    ..Default::default()
                }],
            )
            .expect("a format with a url makes an item"),
        ];

        verify_all(&reqwest::Client::new(), &mut items, &Vec::new()).await;

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].verified, None, "a clip is neither confirmed nor condemned here");
    }

    #[test]
    fn a_video_item_takes_its_shape_from_its_best_format() {
        let item = ImageItem::video(
            "1",
            vec![crate::video::VideoFormat {
                container: "mp4".into(),
                width: Some(1280),
                height: Some(720),
                video_url: Some("https://cdn.example/720.mp4".into()),
                ..Default::default()
            }],
        )
        .unwrap();

        assert_eq!(item.kind, MediaKind::Video);
        assert_eq!(item.url, "https://cdn.example/720.mp4");
        assert_eq!(item.extension, "mp4");
        assert_eq!((item.width, item.height), (Some(1280), Some(720)));
    }

    #[test]
    fn a_clip_with_no_fetchable_rendition_is_not_offered_at_all() {
        // Better no tile than a tile whose download cannot work.
        assert!(ImageItem::video("1", Vec::new()).is_none());
        assert!(
            ImageItem::video("1", vec![crate::video::VideoFormat::default()]).is_none(),
            "a format without a url is not fetchable"
        );
    }

    #[test]
    fn merging_is_case_insensitive_about_header_names() {
        let base = vec![("user-agent".to_string(), "Mozilla/5.0".to_string())];
        let item = vec![("User-Agent".to_string(), "crawler".to_string())];

        let merged = merge_headers(&base, &item);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].1, "crawler");
    }
}
