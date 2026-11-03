//! Ask an mp4 what tracks it actually has, by reading its header.
//!
//! Facebook and Instagram serve a DASH video as two separate `.mp4` files on
//! the same CDN — one all picture, one all sound — and nothing in either URL
//! distinguishes them. When such a URL arrives on its own (pasted, or captured
//! off the wire by the browser extension) there is no manifest to consult, and
//! guessing has exactly the failure the user sees: a silent video, or an audio
//! file with a `.mp4` name.
//!
//! So we look. An ISO base media file names its tracks in
//! `moov/trak/mdia/hdlr`, a few hundred bytes into a header that is almost
//! always at the front of the file; two ranged GETs are enough to read it, and
//! usually one is. This is a classification, not a download — nothing here
//! fetches the media itself.

use reqwest::Client;
use futures_util::StreamExt;

use crate::headers::{self, Headers};

/// How much of the head to ask for on the first request. Large enough to hold
/// the `moov` of an ordinary clip, small enough not to be a download.
const HEAD_BYTES: u64 = 512 * 1024;

/// How much of the tail to ask for when `moov` is not at the front (a file
/// written without `-movflags +faststart`).
const TAIL_BYTES: u64 = 1024 * 1024;

/// A `moov` bigger than this is not worth chasing — at that size the file is
/// long enough that its first bytes told us what we needed anyway.
const MAX_MOOV_BYTES: u64 = 2 * 1024 * 1024;

/// What one stream turned out to contain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Tracks {
    pub video: bool,
    pub audio: bool,
}

impl Tracks {
    pub fn is_video_only(&self) -> bool {
        self.video && !self.audio
    }

    pub fn is_audio_only(&self) -> bool {
        self.audio && !self.video
    }
}

/// Classify the stream at `url`, or `None` if it could not be read as an mp4.
///
/// `None` is the honest answer for a WebM, an HLS playlist, a CDN that refuses
/// ranged requests, or anything else this cannot parse — callers treat it as
/// "unknown" and carry on, never as "has no audio".
pub async fn probe(client: &Client, url: &str, headers: &Headers) -> Option<Tracks> {
    let head = fetch_range(client, url, headers, 0, HEAD_BYTES - 1).await?;

    match find_moov(&head) {
        // The whole header came back in the first request — the common case,
        // and the only one that costs a single round trip.
        Some(Located::Complete(payload)) => return tracks_in_moov(payload),

        // We know where `moov` is and how big it is, so ask for exactly it.
        Some(Located::Truncated { offset, size }) if size <= MAX_MOOV_BYTES => {
            let body = fetch_range(client, url, headers, offset, offset + size - 1).await?;
            let payload = moov_payload_at_start(&body)?;
            return tracks_in_moov(payload);
        }
        Some(Located::Truncated { .. }) => return None,

        // Not at the front: the file was not written for streaming and its
        // header sits at the end.
        None => {}
    }

    let tail = fetch_range_suffix(client, url, headers, TAIL_BYTES).await?;
    // The tail starts mid-file, so the box chain is not aligned to it — scan
    // for the `moov` signature instead of walking from a known boundary.
    tracks_in_moov(find_moov_signature(&tail)?)
}

/// Where a `moov` box was found relative to the buffer it was searched in.
enum Located<'a> {
    /// Its payload, entirely present.
    Complete(&'a [u8]),
    /// Its absolute offset and total size — it starts inside the buffer but
    /// runs past the end of it.
    Truncated { offset: u64, size: u64 },
}

/// Walk the top-level box chain looking for `moov`.
///
/// Only valid on a buffer that starts at byte 0 of the file, where the chain is
/// aligned. A box declaring a size of 0 runs to the end of the file, which ends
/// the walk either way.
fn find_moov(buf: &[u8]) -> Option<Located<'_>> {
    let mut pos: usize = 0;
    while pos + 8 <= buf.len() {
        let (size, header_len) = box_size_at(buf, pos)?;
        let kind = &buf[pos + 4..pos + 8];

        if kind == b"moov" {
            let payload_start = pos + header_len;
            // Size 0 means the box runs to the end of the file, so whatever
            // came back is all of it we are going to get.
            if size == 0 {
                return buf.get(payload_start..).map(Located::Complete);
            }
            let end = (pos as u64).checked_add(size)? as usize;
            return Some(if end <= buf.len() {
                Located::Complete(&buf[payload_start..end])
            } else {
                Located::Truncated {
                    offset: pos as u64,
                    size,
                }
            });
        }

        if size == 0 {
            return None; // runs to EOF, and it is not `moov`
        }
        pos = pos.checked_add(size as usize)?;
    }
    None
}

/// The payload of a `moov` box that begins at byte 0 of `buf`.
fn moov_payload_at_start(buf: &[u8]) -> Option<&[u8]> {
    if buf.len() < 8 || &buf[4..8] != b"moov" {
        return None;
    }
    let (size, header_len) = box_size_at(buf, 0)?;
    let end = std::cmp::min(size as usize, buf.len());
    buf.get(header_len..end)
}

/// Find a `moov` box in a buffer whose start is not a box boundary, by looking
/// for the type field and checking that the size in front of it is plausible.
fn find_moov_signature(buf: &[u8]) -> Option<&[u8]> {
    let mut i = 4;
    while i + 4 <= buf.len() {
        if &buf[i..i + 4] == b"moov" {
            let start = i - 4;
            if let Some((size, header_len)) = box_size_at(buf, start) {
                // A `moov` is never a handful of bytes; a coincidental match on
                // the four characters inside some other payload almost always
                // fails this.
                if size >= 16 {
                    let end = std::cmp::min((start as u64 + size) as usize, buf.len());
                    return buf.get(start + header_len..end);
                }
            }
        }
        i += 1;
    }
    None
}

/// `(total box size, header length)` for the box starting at `pos`.
///
/// A size of 1 means the real size is a 64-bit value after the type; a size of
/// 0 means "to the end of the file", which is reported as 0 for the caller to
/// interpret.
fn box_size_at(buf: &[u8], pos: usize) -> Option<(u64, usize)> {
    let short = u32::from_be_bytes(buf.get(pos..pos + 4)?.try_into().ok()?) as u64;
    match short {
        1 => {
            let large = u64::from_be_bytes(buf.get(pos + 8..pos + 16)?.try_into().ok()?);
            (large >= 16).then_some((large, 16))
        }
        0 => Some((0, 8)),
        n if n >= 8 => Some((n, 8)),
        _ => None,
    }
}

/// Which handler types appear anywhere inside a `moov` payload.
///
/// `hdlr` is nested three levels down (`trak/mdia/hdlr`) and the intervening
/// boxes carry nothing we need, so this scans for the handler declarations
/// directly rather than descending the tree — the four-byte type is preceded by
/// its own box header, which is what keeps the match from being a coincidence.
fn tracks_in_moov(payload: &[u8]) -> Option<Tracks> {
    let mut tracks = Tracks::default();

    let mut i = 4;
    while i + 16 <= payload.len() {
        // The four characters have to be a box *type*, which means the four
        // bytes in front of them are that box's length. Requiring a plausible
        // length is what separates a real `hdlr` from the same letters
        // appearing inside some codec's private data.
        let plausible = payload
            .get(i - 4..i)
            .and_then(|s| s.try_into().ok())
            .map(|s: [u8; 4]| u32::from_be_bytes(s) >= 24)
            .unwrap_or(false);

        if plausible && &payload[i..i + 4] == b"hdlr" {
            // hdlr payload: version+flags (4), pre_defined (4), handler_type (4)
            match payload.get(i + 12..i + 16) {
                Some(b"vide") => tracks.video = true,
                Some(b"soun") => tracks.audio = true,
                _ => {}
            }
            i += 16;
            continue;
        }
        i += 1;
    }

    (tracks.video || tracks.audio).then_some(tracks)
}

async fn fetch_range(
    client: &Client,
    url: &str,
    headers: &Headers,
    start: u64,
    end: u64,
) -> Option<Vec<u8>> {
    send(client, url, headers, format!("bytes={start}-{end}")).await
}

async fn fetch_range_suffix(client: &Client, url: &str, headers: &Headers, len: u64) -> Option<Vec<u8>> {
    send(client, url, headers, format!("bytes=-{len}")).await
}

async fn send(client: &Client, url: &str, headers: &Headers, range: String) -> Option<Vec<u8>> {
    let limit = if let Some(suffix) = range.strip_prefix("bytes=-") {
        suffix.parse::<usize>().ok()?
    } else {
        let (start, end) = range.strip_prefix("bytes=")?.split_once('-')?;
        end.parse::<usize>().ok()?.checked_sub(start.parse::<usize>().ok()?)?.checked_add(1)?
    }.min(MAX_MOOV_BYTES as usize);
    let mut req = client.get(url).header("Range", range).timeout(std::time::Duration::from_secs(3));
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("user-agent")) {
        req = req.header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
        );
    }
    let response = headers::apply(req, headers).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.ok()?;
        let remaining = limit.saturating_sub(bytes.len());
        bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if bytes.len() >= limit { break; }
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_server_ignoring_range_cannot_make_the_probe_buffer_a_whole_file() {
        let app = axum::Router::new().route("/clip.mp4", axum::routing::get(|| async { vec![7u8; 1024 * 1024] }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        let bytes = send(&Client::new(), &format!("http://{address}/clip.mp4"), &Vec::new(), "bytes=0-31".into()).await.unwrap();
        assert_eq!(bytes.len(), 32);
        task.abort();
    }

    /// A box: 4-byte big-endian size, 4-byte type, payload.
    fn mp4_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let size = (8 + payload.len()) as u32;
        let mut out = size.to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }

    /// A `hdlr` box declaring one handler type.
    fn hdlr(handler: &[u8; 4]) -> Vec<u8> {
        let mut payload = vec![0u8; 8]; // version+flags, pre_defined
        payload.extend_from_slice(handler);
        payload.extend_from_slice(b"Handler\0");
        mp4_box(b"hdlr", &payload)
    }

    fn trak(handler: &[u8; 4]) -> Vec<u8> {
        let mdia = mp4_box(b"mdia", &hdlr(handler));
        mp4_box(b"trak", &mdia)
    }

    fn file_with(traks: &[&[u8; 4]], faststart: bool) -> Vec<u8> {
        let mut moov_payload = Vec::new();
        for handler in traks {
            moov_payload.extend_from_slice(&trak(handler));
        }
        let moov = mp4_box(b"moov", &moov_payload);
        let ftyp = mp4_box(b"ftyp", b"isom\0\0\x02\0isomiso2avc1mp41");
        let mdat = mp4_box(b"mdat", &vec![0x11; 4096]);

        let mut out = ftyp;
        if faststart {
            out.extend_from_slice(&moov);
            out.extend_from_slice(&mdat);
        } else {
            out.extend_from_slice(&mdat);
            out.extend_from_slice(&moov);
        }
        out
    }

    fn classify(bytes: &[u8]) -> Option<Tracks> {
        match find_moov(bytes)? {
            Located::Complete(payload) => tracks_in_moov(payload),
            Located::Truncated { .. } => None,
        }
    }

    #[test]
    fn a_muxed_file_reports_both_tracks() {
        let tracks = classify(&file_with(&[b"vide", b"soun"], true)).unwrap();
        assert_eq!(tracks, Tracks { video: true, audio: true });
        assert!(!tracks.is_video_only());
        assert!(!tracks.is_audio_only());
    }

    #[test]
    fn a_dash_video_representation_reports_no_audio() {
        // The Instagram case: picture with nothing to hear.
        let tracks = classify(&file_with(&[b"vide"], true)).unwrap();
        assert!(tracks.is_video_only());
    }

    #[test]
    fn a_dash_audio_representation_reports_no_video() {
        // The Facebook case: the file that arrived instead of the video.
        let tracks = classify(&file_with(&[b"soun"], true)).unwrap();
        assert!(tracks.is_audio_only());
    }

    #[test]
    fn a_moov_at_the_end_is_found_by_signature() {
        let file = file_with(&[b"vide", b"soun"], false);

        // What the head request actually sees: `ftyp`, then an `mdat` that runs
        // past the end of the range. The walk stops there, which is what sends
        // `probe` after the tail.
        let head = &file[..64];
        assert!(find_moov(head).is_none());

        // The tail begins mid-`mdat`, so there is no box boundary to walk from.
        let tail = &file[file.len() - 200..];
        let payload = find_moov_signature(tail).unwrap();
        assert_eq!(tracks_in_moov(payload), Some(Tracks { video: true, audio: true }));
    }

    #[test]
    fn a_head_that_stops_inside_moov_reports_where_to_look() {
        let file = file_with(&[b"vide", b"soun"], true);
        let ftyp_len = 8 + 24;
        // Cut a few bytes into the moov box.
        let truncated = &file[..ftyp_len + 12];
        match find_moov(truncated) {
            Some(Located::Truncated { offset, size }) => {
                assert_eq!(offset, ftyp_len as u64);
                assert!(size > 12, "the box header still carries its real length");
                let payload = moov_payload_at_start(&file[offset as usize..]).unwrap();
                assert_eq!(tracks_in_moov(payload), Some(Tracks { video: true, audio: true }));
            }
            other => panic!("expected a truncated moov, got {}", other.is_some()),
        }
    }

    #[test]
    fn something_that_is_not_an_mp4_classifies_as_unknown() {
        assert!(classify(b"#EXTM3U\n#EXT-X-VERSION:3\n").is_none());
        assert!(classify(b"").is_none());
        // A box chain with no moov in it.
        assert!(classify(&mp4_box(b"ftyp", b"isom")).is_none());
    }

    #[test]
    fn a_sixty_four_bit_box_header_is_understood() {
        // size=1 means the real size follows the type as a u64.
        let payload = hdlr(b"soun");
        let mut boxed = 1u32.to_be_bytes().to_vec();
        boxed.extend_from_slice(b"moov");
        boxed.extend_from_slice(&((16 + payload.len()) as u64).to_be_bytes());
        boxed.extend_from_slice(&payload);

        let tracks = classify(&boxed).unwrap();
        assert!(tracks.is_audio_only());
    }
}
