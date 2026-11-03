//! Fetching one elementary stream referenced by a [`super::VideoFormat`]'s
//! `video_url`/`audio_url`: a direct file, an HLS media playlist, or the
//! pre-resolved DASH segment list [`super::dash`] already expanded into a
//! marker string (`analyze` is stateless, so anything the download step
//! needs must round-trip through that one field).

use std::io::SeekFrom;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::headers::{self, Headers};
use crate::retry::RetryPolicy;
use crate::speed::SharedRateLimit;

const DASH_SEGMENTS_PREFIX: &str = "zoltraak-dash-segments:";
const LEGACY_DASH_SEGMENTS_PREFIX: &str = "opendm-dash-segments:";

/// One piece of a stream to fetch and append.
///
/// Most segments are whole objects (a TS segment, an expanded DASH segment).
/// But fMP4 HLS — what X/Twitter, and many others, serve — packs the init
/// segment and every media segment as byte-range *slices of one file*, declared
/// with `#EXT-X-BYTERANGE`. Fetching the whole URL for each of those and
/// concatenating produces a file full of duplicated, overlapping `moof`/`mdat`
/// boxes that ffmpeg rejects with "no tfhd was found" / "could not find
/// corresponding trex". Carrying the range here lets `download_concat` request
/// exactly the slice each entry names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub url: String,
    /// `(offset, length)` when this segment is a byte-range slice of `url`;
    /// `None` to fetch the whole object.
    pub range: Option<(u64, u64)>,
}

impl Segment {
    /// A whole-object segment — a plain TS/segment URL or an expanded DASH URL.
    pub fn whole(url: String) -> Self {
        Self { url, range: None }
    }
}

/// Encode a resolved list of segment URLs (init segment first, if any) into
/// a single string that fits in `VideoFormat::video_url`/`audio_url`.
pub fn encode_segments(urls: &[String]) -> String {
    format!(
        "{DASH_SEGMENTS_PREFIX}{}",
        serde_json::to_string(urls).expect("Vec<String> always serializes")
    )
}

fn decode_segments(marker: &str) -> Option<Vec<String>> {
    marker
        .strip_prefix(DASH_SEGMENTS_PREFIX)
        .or_else(|| marker.strip_prefix(LEGACY_DASH_SEGMENTS_PREFIX))
        .and_then(|json| serde_json::from_str(json).ok())
}

fn apply_stream_headers(builder: reqwest::RequestBuilder, url: &str, headers: &Headers) -> reqwest::RequestBuilder {
    let mut req = headers::apply(builder, headers);
    let has_ua = headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("user-agent"));
    if !has_ua {
        req = req.header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
        );
    }
    let has_referer = headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("referer"));
    if !has_referer {
        let lower = url.to_ascii_lowercase();
        if lower.contains("googlevideo.com") || lower.contains("youtube.com") || lower.contains("youtu.be") {
            req = req.header("Referer", "https://www.youtube.com/")
                     .header("Origin", "https://www.youtube.com");
        } else if lower.contains("fbcdn.net") || lower.contains("facebook.com") || lower.contains("cdninstagram.com") || lower.contains("instagram.com") {
            req = req.header("Referer", "https://www.instagram.com/")
                     .header("Origin", "https://www.instagram.com");
        } else if lower.contains("tiktokcdn") || lower.contains("tiktok.com") || lower.contains("tiktokv") || lower.contains("bytefcdn") || lower.contains("bytefed") || lower.contains("ibytedtos") || lower.contains("tikwm") {
            req = req.header("Referer", "https://www.tiktok.com/")
                     .header("Origin", "https://www.tiktok.com");
        } else if lower.contains("twimg.com") || lower.contains("twitter.com") || lower.contains("x.com") {
            req = req.header("Referer", "https://x.com/")
                     .header("Origin", "https://x.com");
        }
    }
    req
}

/// One progress report from a running stream.
///
/// `total` is carried alongside the byte count because only the fetcher can
/// know it. For a single file it is the `Content-Length`; for a segment list
/// there is no declared size anywhere, and the only honest number is an
/// estimate extrapolated from the segments already fetched. A caller that had
/// to guess would have nothing to guess from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamTick {
    /// Bytes of this stream received so far, including anything a resume
    /// started from.
    pub bytes: u64,
    /// Best available estimate of this stream's final size, if any.
    pub total: Option<u64>,
}

pub type ProgressCb<'a> = &'a (dyn Fn(StreamTick) + Send + Sync);

/// Where a stream download stopped, and what it takes to continue.
///
/// The unit of resumption is a whole segment, not a byte: a segment URL is not
/// guaranteed to honour a `Range`, and half a segment is not a thing the
/// concatenation can start from. For a single-file source there are no
/// segments, and `bytes` is a real byte offset resumed with an HTTP `Range`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamProgress {
    /// Whole segments already on disk. Always 0 for a single-file source.
    pub segments_done: usize,
    /// Length of the file on disk at that boundary.
    pub bytes: u64,
    /// Identifies the segment list this progress was measured against.
    ///
    /// A playlist can change underneath a paused job — a live stream rolls its
    /// window, a rendition gets re-encoded — and then segment index N no longer
    /// names the same bytes. Resuming against a different list would splice two
    /// unrelated streams together into a file that plays as garbage, so a
    /// mismatch restarts from zero instead.
    pub fingerprint: Option<String>,
}

impl StreamProgress {
    fn is_fresh_start(&self) -> bool {
        self.segments_done == 0 && self.bytes == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamOutcome {
    Completed { bytes: u64 },
    /// Cancelled cleanly at a resumable boundary. Everything counted in the
    /// returned progress is on disk.
    Paused(StreamProgress),
    /// The origin stopped serving the URL (403/410) part-way through a
    /// segment list. Everything counted in `progress` is on disk, so a caller
    /// that can obtain a fresh URL for the same bytes — a signed CDN link
    /// that expired, or one that only serves a bounded prefix — continues
    /// from there instead of starting over.
    Refused { progress: StreamProgress, status: u16, url: String },
}

impl StreamOutcome {
    /// Bytes on disk either way.
    pub fn bytes(&self) -> u64 {
        match self {
            Self::Completed { bytes } => *bytes,
            Self::Paused(progress) => progress.bytes,
            Self::Refused { progress, .. } => progress.bytes,
        }
    }

    pub fn is_paused(&self) -> bool {
        matches!(self, Self::Paused(_))
    }
}

/// A stable identity for an ordered segment list.
///
/// Content-bearing query strings are retained. Known Google Video transport
/// fields can rotate without changing identity; unknown CDN query changes
/// conservatively restart a segmented download rather than splice content.
///
/// FNV-1a rather than `DefaultHasher`, whose output is explicitly not
/// guaranteed stable between Rust releases — this value is written to disk and
/// compared after an upgrade.
pub fn fingerprint(segments: &[Segment]) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = OFFSET;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(PRIME);
        }
    };

    for segment in segments {
        let identity = super::validation::source_identity(&segment.url);
        feed(identity.as_bytes());
        if let Some((offset, length)) = segment.range {
            feed(&offset.to_le_bytes());
            feed(&length.to_le_bytes());
        }
        // A separator no URL can contain, so "a/b" + "c" cannot collide with
        // "a" + "b/c".
        feed(&[0x1f]);
    }

    format!("{}-{hash:016x}", segments.len())
}

/// Download one stream to `dest`, dispatching on what kind of source it is.
#[allow(dead_code)]
pub async fn fetch(client: &Client, source: &str, headers: &Headers, dest: &Path) -> Result<StreamOutcome> {
    fetch_with_progress(client, source, headers, dest, None, &StreamOptions::default()).await
}

pub async fn fetch_with_progress(
    client: &Client,
    source: &str,
    headers: &Headers,
    dest: &Path,
    on_progress: Option<ProgressCb<'_>>,
    options: &StreamOptions,
) -> Result<StreamOutcome> {
    if let Some(json) = source.strip_prefix(super::groups::PREFIX) {
        let groups: Vec<Vec<Segment>> = serde_json::from_str(json)
            .map_err(|_| Error::InvalidInput("Invalid grouped stream plan".into()))?;
        return super::groups::download(client, &groups, headers, dest, on_progress, options).await;
    }
    if let Some(segments) = decode_segments(source) {
        // Expanded DASH segments are always whole objects.
        let segments: Vec<Segment> = segments.into_iter().map(Segment::whole).collect();
        return download_concat_with_options(client, &segments, headers, dest, on_progress, options).await;
    }
    if let Some(plan) = super::youtube::chunk_plan(source) {
        return download_ranged_with_options(client, source, &plan, headers, dest, on_progress, options).await;
    }
    // Match `.m3u8` on the path only — these URLs routinely carry a query
    // string (auth tokens etc.), which a bare `ends_with` would miss, sending
    // the playlist text down the single-file path instead of expanding it.
    let path = source.split(['?', '#']).next().unwrap_or(source);
    if path.to_ascii_lowercase().ends_with(".m3u8") {
        return super::hls::download_media_playlist_with_progress(client, source, headers, dest, on_progress, options).await;
    }
    download_single_with_progress(client, source, headers, dest, on_progress, options).await
}

#[allow(dead_code)]
pub async fn download_single(client: &Client, url: &str, headers: &Headers, dest: &Path) -> Result<StreamOutcome> {
    download_single_with_progress(client, url, headers, dest, None, &StreamOptions::default()).await
}

pub async fn download_single_with_progress(
    client: &Client,
    url: &str,
    headers: &Headers,
    dest: &Path,
    on_progress: Option<ProgressCb<'_>>,
    options: &StreamOptions,
) -> Result<StreamOutcome> {
    // A single file has no segments, so resumption is a plain HTTP `Range`.
    // The server gets the final say: a `206` means it honoured the offset, a
    // `200` means it is sending the whole body again and whatever is on disk
    // has to be discarded rather than appended to.
    #[derive(Serialize, Deserialize)]
    struct SingleIdentity {
        source: String,
        etag: Option<String>,
        total: Option<u64>,
    }
    let stored: Option<SingleIdentity> = options.resume.fingerprint.as_deref()
        .and_then(|s| s.strip_prefix("file-v2:"))
        .and_then(|s| serde_json::from_str(s).ok());
    // Legacy/no-validator progress is restarted. An offset alone cannot prove
    // that today's remote bytes belong to yesterday's partial file.
    let source = super::validation::source_identity(url);
    let resume_from = if stored.as_ref().is_some_and(|s| s.source == source && s.etag.is_some()) {
        options.resume.bytes
    } else { 0 };
    let mut request = apply_stream_headers(client.get(url), url, headers);
    if resume_from > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={resume_from}-"));
        request = request.header(reqwest::header::IF_RANGE, stored.as_ref().unwrap().etag.as_ref().unwrap());
    }

    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::UnexpectedStatus {
            status: status.as_u16(),
            url: url.to_string(),
        });
    }
    let appending = resume_from > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT;

    // On a 206 the length describes the remainder, so the offset we resumed
    // from has to be added back to get the size of the whole file.
    let declared_total = response
        .content_length()
        .map(|len| if appending { len + resume_from } else { len });
    let etag = response.headers().get(reqwest::header::ETAG).and_then(|v| v.to_str().ok())
        .filter(|v| !v.starts_with("W/")).map(str::to_string);
    if appending {
        let range = response.headers().get(reqwest::header::CONTENT_RANGE).and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("bytes ")).and_then(|v| v.split_once('/'));
        let start = range.and_then(|(part, _)| part.split_once('-')).and_then(|(start, _)| start.parse::<u64>().ok());
        let total = range.and_then(|(_, total)| total.parse::<u64>().ok());
        let prior = stored.as_ref().unwrap();
        if start != Some(resume_from) || etag != prior.etag
            || (prior.total.is_some() && prior.total != total) {
            return Err(Error::StaleResume);
        }
    }
    let identity = format!("file-v2:{}", serde_json::to_string(&SingleIdentity {
        source, etag, total: declared_total,
    }).map_err(|e| Error::Corrupt(e.to_string()))?);

    // A video stream never comes back as an image or an HTML document. When it
    // does, the URL was a poster/thumbnail or an expired link that the CDN
    // answered with a 200 error page — saving either as a `.mp4` yields the
    // "downloaded only the thumbnail / a single frame" bug. Reject it with a
    // clear message instead of writing a file that will not play. Missing or
    // generic types (octet-stream, video/*, application/*) are allowed through.
    if let Some(content_type) = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        let ct = content_type.to_ascii_lowercase();
        // A GIF/APNG/WebP the user asked for by name is the clip itself; the
        // destination's extension records that the analysis chose such a container.
        let wants_animated_image = dest
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| super::direct::is_animated_image_container(&e.to_ascii_lowercase()));
        let animated_type = matches!(
            ct.split(';').next().unwrap_or("").trim(),
            "image/gif" | "image/apng" | "image/vnd.mozilla.apng" | "image/webp"
        );
        if ct.starts_with("image/") && !(wants_animated_image && animated_type) {
            return Err(Error::InvalidInput(format!(
                "the source returned an image, not a video ({content_type}) — this looks like a thumbnail/poster rather than the clip: {url}"
            )));
        }
        if ct.starts_with("text/html") {
            return Err(Error::InvalidInput(format!(
                "the source returned a web page, not a video ({content_type}) — the link may have expired or need a logged-in browser session: {url}"
            )));
        }
    }

    let mut file = if appending {
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(dest)
            .await
            .map_err(|source| Error::Io { path: dest.to_path_buf(), source })?;
        // Trim anything past the offset we asked to continue from: a crash can
        // leave a partial write that the server is about to send again.
        file.set_len(resume_from).await.map_err(|source| Error::Io { path: dest.to_path_buf(), source })?;
        let mut file = file;
        file.seek(SeekFrom::End(0)).await.map_err(|source| Error::Io { path: dest.to_path_buf(), source })?;
        file
    } else {
        tokio::fs::File::create(dest).await.map_err(|source| Error::Io {
            path: dest.to_path_buf(),
            source,
        })?
    };

    let mut stream = response.bytes_stream();
    let mut total = if appending { resume_from } else { 0 };

    loop {
        let next = tokio::select! {
            biased;
            _ = options.cancel.cancelled() => None,
            item = stream.next() => item,
        };
        let Some(item) = next else {
            if options.cancel.is_cancelled() {
                file.flush().await.map_err(|source| Error::Io { path: dest.to_path_buf(), source })?;
                return Ok(StreamOutcome::Paused(StreamProgress {
                    segments_done: 0,
                    bytes: total,
                    fingerprint: Some(identity.clone()),
                }));
            }
            break;
        };
        let bytes = item?;

        // Same throttle as the segment path: a plain `.mp4` from a CDN is just
        // as much of the user's bandwidth as an HLS rendition, and a cap that
        // applied to only one of them would be a cap the user cannot predict.
        if let Some(limiter) = &options.limiter {
            let mut guard = limiter.lock().await;
            let delay = guard.delay_for(bytes.len() as u64, Instant::now());
            if delay > Duration::ZERO {
                tokio::time::sleep(delay).await;
            }
        }

        file.write_all(&bytes).await.map_err(|source| Error::Io {
            path: dest.to_path_buf(),
            source,
        })?;
        total += bytes.len() as u64;
        if let Some(cb) = on_progress {
            cb(StreamTick {
                bytes: total,
                total: declared_total,
            });
        }
    }

    file.flush().await.map_err(|source| Error::Io {
        path: dest.to_path_buf(),
        source,
    })?;
    Ok(StreamOutcome::Completed { bytes: total })
}

/// How a segment list is fetched.
#[derive(Debug, Clone)]
pub struct SegmentOptions {
    /// Segments in flight at once. Results are still *written* in playlist
    /// order — see [`download_concat_with_options`] for why order is not
    /// negotiable even though the fetching is unordered.
    pub concurrency: usize,
    /// Per-segment retry, deliberately far tighter than [`RetryPolicy::default`].
    ///
    /// The default policy is tuned for retrying a whole download: five attempts
    /// starting at a five-second backoff. Applied per segment, that would stall
    /// a thousand-segment stream for minutes on one flaky CDN edge. A segment is
    /// small and cheap to re-request, so it gets a few fast attempts instead.
    pub retry: RetryPolicy,
}

impl Default for SegmentOptions {
    fn default() -> Self {
        Self {
            concurrency: 6,
            retry: RetryPolicy {
                max_attempts: 3,
                base: Duration::from_millis(400),
                max_delay: Duration::from_secs(5),
            },
        }
    }
}

/// Everything the download step needs beyond the URL itself.
#[derive(Debug, Clone, Default)]
pub struct StreamOptions {
    /// Explicit bounded HLS recording; absent means capture one snapshot.
    pub live_seconds: Option<u32>,
    pub segments: SegmentOptions,
    /// The app-wide bandwidth cap. `None` leaves this stream unthrottled, which
    /// is only correct for a caller that genuinely has no cap — not for one
    /// that simply has not been wired up.
    pub limiter: Option<SharedRateLimit>,
    /// Where to pick up. The default starts from the beginning.
    pub resume: StreamProgress,
    /// Cancelling this stops at the next resumable boundary and returns
    /// [`StreamOutcome::Paused`] — not an error. A user pausing a download is
    /// not a failure, and the distinction is what lets the job be resumed
    /// rather than retried from zero.
    pub cancel: CancellationToken,
}

/// Fetch every segment and append the bodies into one file — how both HLS
/// segments and expanded DASH segments become a single playable stream.
///
/// Byte-concatenation only works because each source is a continuous rendition
/// with no per-segment remuxing need (see the `hls`/`dash` module docs for what
/// that rules out). That makes **write order load-bearing**: segment N+1's
/// bytes are meaningless anywhere but immediately after segment N's. So
/// segments are fetched concurrently and handed back in playlist order —
/// `buffered` holds a completed-but-early segment until its turn comes, which
/// also bounds peak memory at roughly `concurrency` segments.
///
/// Concurrency is the whole point. A one-hour HLS stream is commonly ~900
/// segments, and fetching them one at a time pays a full round trip before each
/// one: the old sequential loop was latency-bound long before it was
/// bandwidth-bound.
///
/// A segment carrying a byte range asks the server for just that slice via a
/// `Range` header. If the server honors it (`206 Partial Content`) the body is
/// exactly the slice; if it ignores the header and returns the whole object
/// (`200`), we slice client-side so the concatenation stays correct either way.
#[allow(dead_code)]
pub async fn download_concat(client: &Client, segments: &[Segment], headers: &Headers, dest: &Path) -> Result<StreamOutcome> {
    download_concat_with_options(client, segments, headers, dest, None, &StreamOptions::default()).await
}

/// How many ranged requests to keep in flight at googlevideo. Its edges
/// tolerate a few parallel connections per client; more than this draws rate
/// limiting rather than speed.
const YT_MAX_CONCURRENCY: usize = 4;

/// Fetch one googlevideo stream as parallel byte ranges.
///
/// The plan's identity — not the URL — names the resume state: a re-resolved
/// stream comes back from a different CDN edge with a new signature and
/// expiry, and the bytes are the same. Hashing the URL would restart every
/// job whose links had expired while it was paused, which is exactly the job
/// that most needs to continue.
pub async fn download_ranged_with_options(
    client: &Client,
    url: &str,
    plan: &super::youtube::ChunkPlan,
    headers: &Headers,
    dest: &Path,
    on_progress: Option<ProgressCb<'_>>,
    options: &StreamOptions,
) -> Result<StreamOutcome> {
    let chunk = plan.chunk_size.max(1);
    let mut segments = Vec::with_capacity((plan.total / chunk + 1) as usize);
    let mut offset = 0;
    while offset < plan.total {
        let length = chunk.min(plan.total - offset);
        segments.push(Segment { url: url.to_string(), range: Some((offset, length)) });
        offset += length;
    }
    let identity = format!("{}-{}-{}", segments.len(), plan.identity, chunk);
    let concurrency = options.segments.concurrency.clamp(1, YT_MAX_CONCURRENCY);
    download_segments(client, &segments, identity, concurrency, Some(plan.total), headers, dest, on_progress, options).await
}

/// Shared state for one concat run: what has arrived, how big the whole thing
/// looks, and where to report both.
struct ConcatProgress<'a> {
    /// Bytes off the wire across every in-flight segment.
    received: AtomicU64,
    /// Extrapolated final size; `0` until at least one segment has landed.
    estimated_total: AtomicU64,
    /// Whether `estimated_total` is exact (a ranged plan of a known size)
    /// rather than extrapolated, in which case it is never revised.
    exact_total: bool,
    callback: Option<ProgressCb<'a>>,
}

impl ConcatProgress<'_> {
    fn report(&self, bytes: u64) {
        if let Some(callback) = self.callback {
            let estimate = self.estimated_total.load(Ordering::Relaxed);
            callback(StreamTick {
                bytes,
                total: (estimate > 0).then_some(estimate),
            });
        }
    }

    /// Re-estimate the whole stream from the segments written so far.
    ///
    /// Segments in a rendition are cut to a target duration, so their sizes
    /// cluster closely enough that the mean is a good predictor. It is rough
    /// after the first segment (an `EXT-X-MAP` init segment is far smaller than
    /// a media segment) and converges quickly after that — which is the right
    /// trade for a progress bar, where a number that tightens beats a spinner
    /// that never commits to anything.
    fn re_estimate(&self, written: u64, segments_done: usize, segment_count: usize) {
        if segments_done == 0 || self.exact_total {
            return;
        }
        let estimate = (written as f64 / segments_done as f64 * segment_count as f64) as u64;
        self.estimated_total.store(estimate.max(written), Ordering::Relaxed);
    }
}

pub async fn download_concat_with_options(
    client: &Client,
    segments: &[Segment],
    headers: &Headers,
    dest: &Path,
    on_progress: Option<ProgressCb<'_>>,
    options: &StreamOptions,
) -> Result<StreamOutcome> {
    let identity = fingerprint(segments);
    let concurrency = options.segments.concurrency.max(1);
    download_segments(client, segments, identity, concurrency, None, headers, dest, on_progress, options).await
}

/// The concat engine behind both segment lists and ranged plans: `identity`
/// names the resume state and `known_total` (when given) is the exact final
/// size.
#[allow(clippy::too_many_arguments)]
async fn download_segments(
    client: &Client,
    segments: &[Segment],
    identity: String,
    concurrency: usize,
    known_total: Option<u64>,
    headers: &Headers,
    dest: &Path,
    on_progress: Option<ProgressCb<'_>>,
    options: &StreamOptions,
) -> Result<StreamOutcome> {
    if segments.is_empty() {
        return Err(Error::InvalidInput("stream has no segments to download".into()));
    }

    let resume = resolve_resume(&options.resume, &identity, segments.len(), dest).await;
    let skip = resume.segments_done;

    let mut file = if resume.is_fresh_start() {
        tokio::fs::File::create(dest).await.map_err(|source| Error::Io {
            path: dest.to_path_buf(),
            source,
        })?
    } else {
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(dest)
            .await
            .map_err(|source| Error::Io { path: dest.to_path_buf(), source })?;
        // Cut back to the last completed segment boundary. A crash can leave a
        // partially written segment past it, and appending after that would
        // splice a fragment into the middle of the stream.
        file.set_len(resume.bytes).await.map_err(|source| Error::Io { path: dest.to_path_buf(), source })?;
        let mut file = file;
        file.seek(SeekFrom::End(0)).await.map_err(|source| Error::Io { path: dest.to_path_buf(), source })?;
        file
    };

    // Bytes off the wire, across every in-flight segment. Progress is reported
    // from here rather than from the write loop below: with N segments in
    // flight the file lags the network by up to N segments, so a bar driven by
    // writes would sit still and then jump. Seeded with what a resumed run
    // already has, so the readout continues rather than restarting at zero.
    let progress = ConcatProgress {
        received: AtomicU64::new(resume.bytes),
        estimated_total: AtomicU64::new(known_total.unwrap_or(0)),
        exact_total: known_total.is_some(),
        callback: on_progress,
    };
    // A resumed run already knows the average segment size from the bytes it
    // brought with it, so the estimate is useful from the first tick.
    progress.re_estimate(resume.bytes, skip, segments.len());

    // The segment is cloned into each future rather than borrowed from the
    // closure argument: a closure returning a future that borrows its argument
    // cannot satisfy the higher-ranked bound `buffered` needs. A `Segment` is a
    // URL and an optional range, so the clone is free next to the request it
    // is about to make.
    let mut fetches = futures_util::stream::iter(segments[skip..].iter().cloned())
        .map(|segment| {
            let progress = &progress;
            async move { fetch_segment(client, &segment, headers, options, progress).await }
        })
        .buffered(concurrency);

    let mut written = resume.bytes;
    let mut segments_done = skip;

    loop {
        // `biased` so cancellation wins a tie: a user who hit pause should not
        // wait on one more segment that happened to be ready in the same poll.
        // Dropping the `next()` future here also drops whatever is still in
        // flight — those bytes are discarded, and the segments they belonged to
        // are re-fetched on resume. That is the price of segment granularity,
        // and it is bounded by `concurrency` segments.
        let next = tokio::select! {
            biased;
            _ = options.cancel.cancelled() => None,
            item = fetches.next() => item,
        };

        let Some(segment) = next else {
            if options.cancel.is_cancelled() {
                file.flush().await.map_err(|source| Error::Io { path: dest.to_path_buf(), source })?;
                return Ok(StreamOutcome::Paused(StreamProgress {
                    segments_done,
                    bytes: written,
                    fingerprint: Some(identity),
                }));
            }
            break;
        };

        let bytes = match segment {
            Ok(bytes) => bytes,
            // Not a failure of this download but of this *link*: what was
            // written in order is good, and a fresh link continues from it.
            Err(Error::UnexpectedStatus { status: status @ (401 | 403 | 410), url }) => {
                file.flush().await.map_err(|source| Error::Io { path: dest.to_path_buf(), source })?;
                return Ok(StreamOutcome::Refused {
                    progress: StreamProgress { segments_done, bytes: written, fingerprint: Some(identity) },
                    status,
                    url,
                });
            }
            Err(e) => return Err(e),
        };
        file.write_all(&bytes).await.map_err(|source| Error::Io {
            path: dest.to_path_buf(),
            source,
        })?;
        written += bytes.len() as u64;
        segments_done += 1;
        progress.re_estimate(written, segments_done, segments.len());
    }

    file.flush().await.map_err(|source| Error::Io {
        path: dest.to_path_buf(),
        source,
    })?;
    Ok(StreamOutcome::Completed { bytes: written })
}

/// Decide whether stored progress can actually be continued, falling back to a
/// fresh start whenever anything about it fails to line up.
///
/// Restarting costs the user time; resuming against the wrong bytes costs them
/// a file that looks complete and does not play. The tie goes to restarting.
async fn resolve_resume(
    stored: &StreamProgress,
    identity: &str,
    segment_count: usize,
    dest: &Path,
) -> StreamProgress {
    let fresh = StreamProgress::default();

    if stored.is_fresh_start() {
        return fresh;
    }
    if stored.segments_done > segment_count {
        tracing::info!("resume state names more segments than the playlist has; restarting");
        return fresh;
    }
    if stored.fingerprint.as_deref() != Some(identity) {
        tracing::info!("playlist changed since this job was paused; restarting");
        return fresh;
    }
    match tokio::fs::metadata(dest).await {
        // The file must still hold at least what the state claims. Shorter
        // means something truncated it, and resuming would leave a hole.
        Ok(meta) if meta.len() >= stored.bytes => stored.clone(),
        Ok(meta) => {
            tracing::info!(
                on_disk = meta.len(),
                expected = stored.bytes,
                "partial file is shorter than the resume state; restarting"
            );
            fresh
        }
        Err(_) => {
            tracing::info!("partial file is gone; restarting");
            fresh
        }
    }
}

/// One segment, retried on transient failures.
///
/// Retrying per segment is what makes the concurrency safe to turn up: more
/// requests in flight means more chances to meet a 503 or a dropped connection,
/// and the previous loop failed the entire job on the first one.
async fn fetch_segment(
    client: &Client,
    segment: &Segment,
    headers: &Headers,
    options: &StreamOptions,
    progress: &ConcatProgress<'_>,
) -> Result<Vec<u8>> {
    let mut attempt = 1u32;
    loop {
        match fetch_segment_once(client, segment, headers, options, progress).await {
            Ok(bytes) => return Ok(bytes),
            Err(error) => {
                if !error.is_retryable() {
                    return Err(error);
                }
                let Some(delay) = options.segments.retry.delay_for(attempt) else {
                    return Err(error);
                };
                tracing::debug!(url = %segment.url, attempt, %error, "segment fetch failed; retrying");
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
        }
    }
}

async fn fetch_segment_once(
    client: &Client,
    segment: &Segment,
    headers: &Headers,
    options: &StreamOptions,
    progress: &ConcatProgress<'_>,
) -> Result<Vec<u8>> {
    let mut request = apply_stream_headers(client.get(&segment.url), &segment.url, headers);
    if let Some((offset, length)) = segment.range {
        let last = offset + length.saturating_sub(1);
        request = request.header(reqwest::header::RANGE, format!("bytes={offset}-{last}"));
    }

    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::UnexpectedStatus {
            status: status.as_u16(),
            url: segment.url.clone(),
        });
    }

    // Slice client-side only when we asked for a range and the server sent the
    // whole object anyway (200 instead of 206). Half-open [start, end).
    let slice = match segment.range {
        Some((offset, length)) if status != reqwest::StatusCode::PARTIAL_CONTENT => {
            Some((offset, offset + length))
        }
        _ => None,
    };

    let mut out = Vec::new();
    let mut attempt_bytes = 0u64;
    let mut stream = response.bytes_stream();
    let mut pos = 0u64;

    let outcome = async {
        while let Some(item) = stream.next().await {
            let bytes = item?;

            // Throttled after the bytes are already off the wire — there is no
            // way to un-receive a TCP payload that has arrived. Delaying here
            // slows the next `stream.next()`, which is what actually applies
            // back pressure. The lock is held across the sleep for the reason
            // given on `RateLimit::delay_for`: releasing it early would let
            // every in-flight segment sleep in parallel, so `concurrency`
            // streams would add up to `concurrency` times the cap.
            if let Some(limiter) = &options.limiter {
                let mut guard = limiter.lock().await;
                let delay = guard.delay_for(bytes.len() as u64, Instant::now());
                if delay > Duration::ZERO {
                    tokio::time::sleep(delay).await;
                }
            }

            let chunk_start = pos;
            let chunk_end = pos + bytes.len() as u64;
            pos = chunk_end;

            let to_write: &[u8] = match slice {
                None => &bytes,
                Some((start, end)) => {
                    let from = start.max(chunk_start);
                    let to = end.min(chunk_end);
                    if from >= to {
                        // Entirely before the slice: skip. Entirely after: stop
                        // — the rest of this response is past what we need.
                        if chunk_start >= end {
                            break;
                        }
                        continue;
                    }
                    &bytes[(from - chunk_start) as usize..(to - chunk_start) as usize]
                }
            };

            out.extend_from_slice(to_write);
            attempt_bytes += to_write.len() as u64;

            let added = to_write.len() as u64;
            let total = progress.received.fetch_add(added, Ordering::Relaxed) + added;
            progress.report(total);
        }
        Ok::<(), Error>(())
    }
    .await;

    if let Err(error) = outcome {
        // This attempt's bytes never reach the file, so un-count them. Without
        // it, a retried segment inflates the progress readout past 100%.
        progress.received.fetch_sub(attempt_bytes, Ordering::Relaxed);
        return Err(error);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_round_trip_through_the_marker_string() {
        let urls = vec!["https://a/1".to_string(), "https://a/2".to_string()];
        let marker = encode_segments(&urls);
        assert!(marker.starts_with(DASH_SEGMENTS_PREFIX));
        assert_eq!(decode_segments(&marker), Some(urls));
    }

    #[test]
    fn a_plain_url_is_not_mistaken_for_a_segment_marker() {
        assert_eq!(decode_segments("https://example.com/video.mp4"), None);
    }
}
