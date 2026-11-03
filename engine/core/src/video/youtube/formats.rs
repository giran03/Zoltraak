//! From InnerTube's `streamingData` to the engine's [`VideoFormat`]s.
//!
//! YouTube describes a video as a ladder of separate video-only and audio-only
//! streams (plus one or two old muxed ones), each in up to three codecs. A
//! download manager wants one entry per quality with sound attached, so this
//! module pairs each video rendition with the right audio track, picks one
//! codec per rung, and reports the combined size.

use serde_json::Value;

use super::super::VideoFormat;

/// One stream as InnerTube lists it, before any challenge is solved.
#[derive(Debug, Clone, PartialEq)]
pub struct RawStream {
    pub itag: u32,
    /// Plain URL, when the client hands one out.
    pub url: Option<String>,
    /// The encrypted alternative: a URL plus a signature to decode and the
    /// query parameter to attach it under.
    pub cipher: Option<Cipher>,
    pub mime: String,
    pub codecs: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f32>,
    pub bitrate: Option<u64>,
    pub content_length: Option<u64>,
    pub quality_label: Option<String>,
    pub audio_quality: Option<String>,
    pub audio_is_default: bool,
    pub language: Option<String>,
    pub is_drc: bool,
    pub has_drm: bool,
    pub is_otf: bool,
    /// Live-only adaptive segments (`targetDurationSec`), not a whole file.
    pub is_live_segmented: bool,
    pub approx_duration_ms: Option<u64>,
    pub adaptive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cipher {
    pub url: String,
    pub s: String,
    /// The query parameter the decoded signature goes under, `sig` by default.
    pub sp: String,
}

impl RawStream {
    pub fn is_video(&self) -> bool {
        self.mime.starts_with("video/")
    }

    pub fn is_audio_only(&self) -> bool {
        self.mime.starts_with("audio/")
    }

    /// A video track that also carries audio: the old-style `formats` entries.
    pub fn is_muxed(&self) -> bool {
        self.is_video() && !self.adaptive
    }

    pub fn container(&self) -> &'static str {
        if self.mime.contains("webm") { "webm" } else { "mp4" }
    }

    /// The URL to fetch before signature/`n` solving: plain or from the cipher.
    pub fn base_url(&self) -> Option<&str> {
        self.url.as_deref().or_else(|| self.cipher.as_ref().map(|c| c.url.as_str()))
    }

    /// Whether the download step can fetch this stream as a whole file.
    pub fn is_downloadable(&self) -> bool {
        self.base_url().is_some() && !self.has_drm && !self.is_otf && !self.is_live_segmented
    }

    /// The `n` throttling challenge in the URL, if the URL has one.
    pub fn n_challenge(&self) -> Option<String> {
        self.base_url().and_then(|u| query_param(u, "n"))
    }

    /// Whether the encoder produced a truncated rendition — a stream whose
    /// duration is far below the video's is corrupt, not short.
    pub fn is_damaged(&self, video_duration_secs: Option<f64>) -> bool {
        match (self.approx_duration_ms, video_duration_secs) {
            (Some(ms), Some(secs)) if secs > 0.0 => (ms as f64 / 1000.0) < (secs / 2.0),
            _ => false,
        }
    }

    fn codec_rank(&self) -> u8 {
        let c = self.codecs.to_ascii_lowercase();
        // H.264 plays everywhere, in hardware, on anything made in the last
        // fifteen years; VP9 nearly so; AV1 is the smallest file and the one
        // most likely to stutter in an older player.
        if c.starts_with("avc1") || c.starts_with("h264") {
            3
        } else if c.starts_with("vp09") || c.starts_with("vp9") {
            2
        } else if c.starts_with("av01") {
            1
        } else {
            0
        }
    }
}

/// The value of a query parameter, percent-decoded.
pub fn query_param(url: &str, name: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    let query = query.split('#').next().unwrap_or(query);
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then(|| percent_decode(v))
    })
}

/// Replace (or add) one query parameter, leaving the rest byte-for-byte as
/// they were — a signed URL must not be re-encoded.
pub fn with_query_param(url: &str, name: &str, value: &str) -> String {
    let (base, fragment) = match url.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (url, None),
    };
    let (path, query) = match base.split_once('?') {
        Some((p, q)) => (p, q),
        None => (base, ""),
    };
    let encoded = percent_encode(value);
    let mut replaced = false;
    let mut pairs: Vec<String> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let key = pair.split('=').next().unwrap_or(pair);
            if key == name {
                replaced = true;
                format!("{name}={encoded}")
            } else {
                pair.to_string()
            }
        })
        .collect();
    if !replaced {
        pairs.push(format!("{name}={encoded}"));
    }
    let mut out = format!("{path}?{}", pairs.join("&"));
    if let Some(f) = fragment {
        out.push('#');
        out.push_str(f);
    }
    out
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(byte) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        if b[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(b[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Parse the `formats` and `adaptiveFormats` arrays of one `streamingData`.
pub fn parse_streams(streaming_data: &Value) -> Vec<RawStream> {
    let mut out = Vec::new();
    for (key, adaptive) in [("formats", false), ("adaptiveFormats", true)] {
        let Some(list) = streaming_data.get(key).and_then(Value::as_array) else { continue };
        for f in list {
            let Some(itag) = f.get("itag").and_then(Value::as_u64) else { continue };
            let mime_full = f.get("mimeType").and_then(Value::as_str).unwrap_or("");
            let (mime, codecs) = split_mime(mime_full);
            let cipher = f
                .get("signatureCipher")
                .or_else(|| f.get("cipher"))
                .and_then(Value::as_str)
                .and_then(parse_cipher);
            let audio_track = f.get("audioTrack");
            out.push(RawStream {
                itag: itag as u32,
                url: f.get("url").and_then(Value::as_str).map(str::to_string),
                cipher,
                mime: mime.to_string(),
                codecs: codecs.to_string(),
                width: f.get("width").and_then(Value::as_u64).map(|n| n as u32),
                height: f.get("height").and_then(Value::as_u64).map(|n| n as u32),
                fps: f.get("fps").and_then(Value::as_f64).map(|n| n as f32).filter(|n| *n > 1.0),
                bitrate: f.get("averageBitrate").or_else(|| f.get("bitrate")).and_then(Value::as_u64),
                content_length: f.get("contentLength").and_then(Value::as_str).and_then(|s| s.parse().ok()),
                quality_label: f.get("qualityLabel").and_then(Value::as_str).map(str::to_string),
                audio_quality: f.get("audioQuality").and_then(Value::as_str).map(str::to_string),
                audio_is_default: audio_track.and_then(|t| t.get("audioIsDefault")).and_then(Value::as_bool).unwrap_or(true),
                language: audio_track.and_then(|t| t.get("id")).and_then(Value::as_str).map(|id| {
                    id.split('.').next().unwrap_or(id).to_string()
                }),
                is_drc: f.get("isDrc").and_then(Value::as_bool).unwrap_or(false),
                has_drm: f.get("drmFamilies").is_some(),
                is_otf: f.get("type").and_then(Value::as_str) == Some("FORMAT_STREAM_TYPE_OTF"),
                is_live_segmented: f.get("targetDurationSec").is_some(),
                approx_duration_ms: f.get("approxDurationMs").and_then(Value::as_str).and_then(|s| s.parse().ok()),
                adaptive,
            });
        }
    }
    out
}

fn split_mime(full: &str) -> (&str, &str) {
    let mut parts = full.splitn(2, ';');
    let mime = parts.next().unwrap_or("").trim();
    let codecs = parts
        .next()
        .and_then(|rest| rest.trim().strip_prefix("codecs="))
        .map(|c| c.trim_matches('"'))
        .unwrap_or("");
    (mime, codecs)
}

fn parse_cipher(raw: &str) -> Option<Cipher> {
    let mut url = None;
    let mut s = None;
    let mut sp = None;
    for pair in raw.split('&') {
        let Some((k, v)) = pair.split_once('=') else { continue };
        match k {
            "url" => url = Some(percent_decode(v)),
            "s" => s = Some(percent_decode(v)),
            "sp" => sp = Some(percent_decode(v)),
            _ => {}
        }
    }
    Some(Cipher { url: url?, s: s?, sp: sp.unwrap_or_else(|| "signature".into()) })
}

/// A stream with its final, fetchable URL.
#[derive(Debug, Clone)]
pub struct ReadyStream {
    pub stream: RawStream,
    pub url: String,
}

/// Turn the resolved streams of one response into the formats a picker shows.
///
/// One entry per (height, frame rate) rung, in the most compatible codec that
/// rung comes in; each video-only rendition paired with the best audio track
/// of the same container family; the muxed legacy formats as they are; and
/// the best audio-only track on its own, for the user who wants the sound.
pub fn build_formats(streams: &[ReadyStream], video_duration: Option<f64>) -> Vec<VideoFormat> {
    let usable: Vec<&ReadyStream> = streams
        .iter()
        .filter(|r| r.stream.is_downloadable() && !r.stream.is_damaged(video_duration))
        .collect();

    // Audio candidates: the default-language, non-DRC tracks.
    let audio: Vec<&ReadyStream> = usable
        .iter()
        .copied()
        .filter(|r| r.stream.is_audio_only() && r.stream.audio_is_default && !r.stream.is_drc)
        .collect();
    let best_audio = |container: &str| -> Option<&ReadyStream> {
        let same_family = audio.iter().copied().filter(|a| a.stream.container() == container).max_by_key(|a| a.stream.bitrate.unwrap_or(0));
        same_family.or_else(|| audio.iter().copied().max_by_key(|a| a.stream.bitrate.unwrap_or(0)))
    };

    let mut formats = Vec::new();

    // Video-only rungs: best codec per (height, fps).
    let mut rungs: Vec<(u32, u32, &ReadyStream)> = Vec::new();
    for r in usable.iter().copied().filter(|r| r.stream.is_video() && r.stream.adaptive) {
        let Some(height) = r.stream.height else { continue };
        let fps = r.stream.fps.map(|f| f.round() as u32).unwrap_or(30);
        match rungs.iter_mut().find(|(h, f, _)| *h == height && *f == fps) {
            Some(existing) => {
                let better = r.stream.codec_rank() > existing.2.stream.codec_rank()
                    || (r.stream.codec_rank() == existing.2.stream.codec_rank()
                        && r.stream.bitrate.unwrap_or(0) > existing.2.stream.bitrate.unwrap_or(0));
                if better {
                    existing.2 = r;
                }
            }
            None => rungs.push((height, fps, r)),
        }
    }
    for (_, _, video) in rungs {
        let Some(audio) = best_audio(video.stream.container()) else {
            // No audio track at all — offer it honestly as silent.
            formats.push(video_format(video, None));
            continue;
        };
        formats.push(video_format(video, Some(audio)));
    }

    // Muxed legacy formats (18/22): complete files, no merge — but the CDN
    // now refuses them without a proof-of-origin token (and they declare no
    // size), so they are a fallback for a video with no adaptive rendition
    // at that height, never the first choice.
    let best_rung_height = formats.iter().filter_map(|f| f.height).max().unwrap_or(0);
    for r in usable.iter().copied().filter(|r| {
        r.stream.is_muxed() && r.stream.height.unwrap_or(0) > best_rung_height
    }) {
        formats.push(VideoFormat {
            validation: None,
            download_options: None,
            id: r.stream.itag.to_string(),
            container: r.stream.container().into(),
            video_codec: first_codec(&r.stream.codecs),
            audio_codec: second_codec(&r.stream.codecs),
            width: r.stream.width,
            height: r.stream.height,
            fps: r.stream.fps,
            bitrate: r.stream.bitrate,
            filesize: r.stream.content_length,
            video_url: Some(r.url.clone()),
            audio_url: None,
            requires_merge: false,
            has_audio: Some(true),
            headers: Vec::new(),
        });
    }

    // Audio only: the best m4a, else the best of whatever there is.
    if let Some(a) = best_audio("mp4") {
        formats.push(VideoFormat {
            validation: None,
            download_options: None,
            id: a.stream.itag.to_string(),
            container: if a.stream.container() == "mp4" { "m4a".into() } else { "webm".into() },
            video_codec: None,
            audio_codec: first_codec(&a.stream.codecs),
            width: None,
            height: None,
            fps: None,
            bitrate: a.stream.bitrate,
            filesize: a.stream.content_length,
            video_url: Some(a.url.clone()),
            audio_url: None,
            requires_merge: false,
            has_audio: Some(true),
            headers: Vec::new(),
        });
    }

    formats
}

fn video_format(video: &ReadyStream, audio: Option<&ReadyStream>) -> VideoFormat {
    VideoFormat {
        validation: None,
        download_options: None,
        id: video.stream.itag.to_string(),
        container: video.stream.container().into(),
        video_codec: first_codec(&video.stream.codecs),
        audio_codec: audio.and_then(|a| first_codec(&a.stream.codecs)),
        width: video.stream.width,
        height: video.stream.height,
        fps: video.stream.fps,
        bitrate: video.stream.bitrate,
        filesize: match audio {
            Some(a) => match (video.stream.content_length, a.stream.content_length) {
                (Some(v), Some(au)) => Some(v + au),
                (v, _) => v,
            },
            None => video.stream.content_length,
        },
        video_url: Some(video.url.clone()),
        audio_url: audio.map(|a| a.url.clone()),
        requires_merge: audio.is_some(),
        has_audio: Some(false),
        headers: Vec::new(),
    }
}

fn first_codec(codecs: &str) -> Option<String> {
    codecs.split(',').next().map(str::trim).filter(|c| !c.is_empty()).map(str::to_string)
}

fn second_codec(codecs: &str) -> Option<String> {
    codecs.split(',').nth(1).map(str::trim).filter(|c| !c.is_empty()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ready(itag: u32, mime: &str, height: Option<u32>, fps: Option<f32>, bitrate: u64, len: u64, adaptive: bool) -> ReadyStream {
        let (m, c) = split_mime(mime);
        ReadyStream {
            url: format!("https://rr1.googlevideo.com/videoplayback?itag={itag}"),
            stream: RawStream {
                itag,
                url: Some(format!("https://rr1.googlevideo.com/videoplayback?itag={itag}")),
                cipher: None,
                mime: m.into(),
                codecs: c.into(),
                width: height.map(|h| h * 16 / 9),
                height,
                fps,
                bitrate: Some(bitrate),
                content_length: Some(len),
                quality_label: None,
                audio_quality: None,
                audio_is_default: true,
                language: None,
                is_drc: false,
                has_drm: false,
                is_otf: false,
                is_live_segmented: false,
                approx_duration_ms: Some(100_000),
                adaptive,
            },
        }
    }

    #[test]
    fn streams_are_parsed_with_ciphers_and_flags() {
        let sd = json!({
            "formats": [{ "itag": 18, "mimeType": "video/mp4; codecs=\"avc1.42001E, mp4a.40.2\"", "url": "https://a/vp?n=abc", "height": 360, "bitrate": 500 }],
            "adaptiveFormats": [
                { "itag": 137, "mimeType": "video/mp4; codecs=\"avc1.640028\"", "signatureCipher": "s=AB%3DCD&sp=sig&url=https%3A%2F%2Fa%2Fvp%3Fn%3Dxyz", "height": 1080, "fps": 30, "contentLength": "1000", "averageBitrate": 4000 },
                { "itag": 140, "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "url": "https://a/vp?n=xyz", "contentLength": "200", "audioTrack": { "id": "en.4", "audioIsDefault": true }, "isDrc": false },
                { "itag": 999, "mimeType": "video/mp4; codecs=\"avc1\"", "url": "https://a/vp", "drmFamilies": ["widevine"] },
                { "itag": 998, "mimeType": "video/mp4; codecs=\"avc1\"", "url": "https://a/vp", "type": "FORMAT_STREAM_TYPE_OTF" }
            ]
        });
        let streams = parse_streams(&sd);
        assert_eq!(streams.len(), 5);
        assert!(streams[0].is_muxed());
        assert_eq!(streams[0].n_challenge().as_deref(), Some("abc"));
        let cipher = streams[1].cipher.as_ref().unwrap();
        assert_eq!(cipher.s, "AB=CD");
        assert_eq!(cipher.sp, "sig");
        assert_eq!(cipher.url, "https://a/vp?n=xyz");
        assert_eq!(streams[1].n_challenge().as_deref(), Some("xyz"));
        assert_eq!(streams[2].language.as_deref(), Some("en"));
        assert!(!streams[3].is_downloadable(), "DRM");
        assert!(!streams[4].is_downloadable(), "OTF");
    }

    #[test]
    fn query_params_are_replaced_without_touching_the_rest() {
        let url = "https://a/vp?expire=1&n=abc&sig=A%3DB#frag";
        assert_eq!(with_query_param(url, "n", "z z"), "https://a/vp?expire=1&n=z%20z&sig=A%3DB#frag");
        assert_eq!(with_query_param("https://a/vp", "pot", "t"), "https://a/vp?pot=t");
        assert_eq!(query_param(url, "sig").as_deref(), Some("A=B"));
        assert_eq!(query_param(url, "nope"), None);
    }

    #[test]
    fn one_rung_per_resolution_prefers_h264_and_pairs_same_family_audio() {
        let streams = vec![
            ready(137, "video/mp4; codecs=\"avc1.640028\"", Some(1080), Some(30.0), 4000, 1000, true),
            ready(248, "video/webm; codecs=\"vp9\"", Some(1080), Some(30.0), 3000, 900, true),
            ready(399, "video/mp4; codecs=\"av01.0.08M.08\"", Some(1080), Some(30.0), 2000, 800, true),
            ready(303, "video/webm; codecs=\"vp9\"", Some(1080), Some(60.0), 5000, 1500, true),
            ready(140, "audio/mp4; codecs=\"mp4a.40.2\"", None, None, 128_000, 200, true),
            ready(251, "audio/webm; codecs=\"opus\"", None, None, 160_000, 220, true),
            ready(18, "video/mp4; codecs=\"avc1.42001E, mp4a.40.2\"", Some(360), Some(30.0), 500, 300, false),
        ];
        let formats = build_formats(&streams, Some(100.0));
        let ids: Vec<&str> = formats.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, vec!["137", "303", "140"], "the muxed 360p is redundant next to adaptive 1080p rungs");

        let f137 = &formats[0];
        assert!(f137.requires_merge);
        assert_eq!(f137.audio_url.as_deref(), Some("https://rr1.googlevideo.com/videoplayback?itag=140"));
        assert_eq!(f137.filesize, Some(1200), "video plus its audio");
        assert_eq!(f137.audio_codec.as_deref(), Some("mp4a.40.2"));
        assert_eq!(f137.has_audio, Some(false));

        let f303 = &formats[1];
        assert_eq!(f303.audio_url.as_deref(), Some("https://rr1.googlevideo.com/videoplayback?itag=251"), "webm pairs with opus");
        assert_eq!(f303.fps, Some(60.0));

        assert_eq!(formats[2].container, "m4a");
        assert_eq!(formats[2].height, None);
        assert_eq!(formats[2].label(), "128kbps");

        // With no adaptive rendition at all, the muxed file is offered.
        let only_muxed = vec![ready(18, "video/mp4; codecs=\"avc1.42001E, mp4a.40.2\"", Some(360), Some(30.0), 500, 300, false)];
        let formats = build_formats(&only_muxed, Some(100.0));
        assert_eq!(formats.len(), 1);
        assert_eq!(formats[0].has_audio, Some(true));
        assert!(!formats[0].requires_merge);
    }

    #[test]
    fn damaged_and_undownloadable_streams_are_left_out() {
        let mut short = ready(137, "video/mp4; codecs=\"avc1\"", Some(1080), None, 1, 1, true);
        short.stream.approx_duration_ms = Some(10_000);
        let mut drm = ready(248, "video/webm; codecs=\"vp9\"", Some(1080), None, 1, 1, true);
        drm.stream.has_drm = true;
        let audio = ready(140, "audio/mp4; codecs=\"mp4a\"", None, None, 1, 1, true);
        let formats = build_formats(&[short, drm, audio], Some(100.0));
        assert_eq!(formats.len(), 1, "only the audio survives: {formats:?}");
    }

    #[test]
    fn percent_decoding_handles_the_usual_suspects() {
        assert_eq!(percent_decode("a%3Db+c%20d"), "a=b c d");
        assert_eq!(percent_decode("plain"), "plain");
        assert_eq!(percent_decode("%zz"), "%zz");
    }
}
