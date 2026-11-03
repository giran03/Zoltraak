//! HLS: parse master and media playlists and resolve segment URIs.
//!
//! Clear segments are grouped at discontinuities and initialization changes.
//! Protected/encrypted segments are rejected before any media is fetched.

use std::collections::HashMap;
use std::path::Path;

use reqwest::Client;

use super::net::{base_url, fetch_text, resolve};
use super::stream::{Segment, StreamOptions, StreamOutcome};
use super::{VideoFormat, VideoMetadata};
use crate::error::{Error, Result};
use crate::headers::Headers;

/// An audio rendition declared by `#EXT-X-MEDIA:TYPE=AUDIO` and referenced by a
/// variant's `AUDIO="group"` — the separate audio track that must be merged
/// back with the video, which is exactly what X, Instagram, and Facebook serve
/// and what the old code silently dropped, producing a video with no sound.
struct AudioRendition {
    url: String,
    codec: Option<String>,
    default: bool,
}

pub async fn analyze(client: &Client, url: &str, headers: &Headers) -> Result<VideoMetadata> {
    let text = fetch_text(client, url, headers).await?;
    let base = base_url(url);
    if !text
        .trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with("#EXTM3U")
    {
        return Err(Error::InvalidInput(
            "The stream endpoint did not return an HLS playlist. Resume playback and retry.".into(),
        ));
    }
    validate_clear_playlist(&text)?;

    let formats = if text.contains("#EXT-X-STREAM-INF") {
        parse_master(&text, &base)
    } else {
        if parse_media_groups(&text, &base)?.is_empty() {
            return Err(Error::InvalidInput(
                "The HLS playlist has no media segments yet. Resume playback and retry.".into(),
            ));
        }
        // Already a media playlist: one rendition, audio assumed muxed in.
        // Container is mp4 because the download step remuxes the concatenated
        // MPEG-TS segments into a clean, universally playable .mp4.
        vec![VideoFormat {
            validation: None,
            download_options: None,
            id: "hls".into(),
            container: "mp4".into(),
            video_codec: None,
            audio_codec: None,
            width: None,
            height: None,
            fps: None,
            bitrate: None,
            filesize: None,
            video_url: Some(url.to_string()),
            audio_url: None,
            has_audio: None,
            headers: Vec::new(),
            requires_merge: false,
        }]
    };

    if formats.is_empty() {
        return Err(Error::InvalidInput(format!(
            "no renditions found in HLS playlist: {url}"
        )));
    }

    Ok(VideoMetadata {
        subtitles: Vec::new(),
        title: super::direct::title_from_url(url),
        thumbnail: None,
        duration: if text.lines().any(|line| line.trim() == "#EXT-X-ENDLIST") {
            let durations: Option<Vec<f64>> = text
                .lines()
                .filter_map(|line| line.trim().strip_prefix("#EXTINF:"))
                .map(|value| {
                    value
                        .split(',')
                        .next()?
                        .parse::<f64>()
                        .ok()
                        .filter(|v| v.is_finite() && *v > 0.0)
                })
                .collect();
            durations
                .filter(|values| !values.is_empty())
                .map(|values| values.iter().sum())
        } else {
            None
        },
        uploader: None,
        webpage_url: url.to_string(),
        formats,
    })
}

#[allow(dead_code)]
pub async fn download_media_playlist(
    client: &Client,
    url: &str,
    headers: &Headers,
    dest: &Path,
) -> Result<StreamOutcome> {
    download_media_playlist_with_progress(
        client,
        url,
        headers,
        dest,
        None,
        &StreamOptions::default(),
    )
    .await
}

pub async fn download_media_playlist_with_progress(
    client: &Client,
    url: &str,
    headers: &Headers,
    dest: &Path,
    on_progress: Option<super::stream::ProgressCb<'_>>,
    options: &StreamOptions,
) -> Result<StreamOutcome> {
    if let Some(seconds) = options.live_seconds {
        return super::live::record(client, url, headers, dest, on_progress, options, seconds)
            .await;
    }
    let text = fetch_text(client, url, headers).await?;
    let base = base_url(url);
    let groups = parse_media_groups(&text, &base)?;

    if groups.is_empty() {
        return Err(Error::InvalidInput(format!(
            "HLS media playlist has no segments: {url}"
        )));
    }

    super::groups::download(client, &groups, headers, dest, on_progress, options).await
}

/// Turn a media playlist into the ordered list of segments to fetch, resolving
/// `#EXT-X-MAP` init segments and `#EXT-X-BYTERANGE` slices.
///
/// A `#EXT-X-BYTERANGE` (or a `BYTERANGE` on the `#EXT-X-MAP`) makes the next
/// URI a slice of that file. Per the HLS spec, when the byte-range's offset is
/// omitted it continues immediately after the previous sub-range of the *same*
/// resource — so a running offset is tracked per URL.
#[cfg(test)]
fn parse_media_segments(text: &str, base: &str) -> Vec<Segment> {
    parse_media_groups(text, base)
        .unwrap()
        .into_iter()
        .flatten()
        .collect()
}

fn parse_media_groups(text: &str, base: &str) -> Result<Vec<Vec<Segment>>> {
    Ok(parse_snapshot(text, base)?.groups)
}

pub(crate) struct LiveEntry {
    pub sequence: u64,
    pub duration: Option<f64>,
    pub segments: Vec<Segment>,
}
pub(crate) struct Snapshot {
    pub entries: Vec<LiveEntry>,
    pub groups: Vec<Vec<Segment>>,
    pub target: f64,
    pub ended: bool,
}

pub(crate) fn parse_snapshot(text: &str, base: &str) -> Result<Snapshot> {
    if !text
        .trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with("#EXTM3U")
    {
        return Err(Error::InvalidInput("Expected an HLS media playlist".into()));
    }
    validate_clear_playlist(text)?;
    let mut groups = Vec::new();
    let mut segments = Vec::new();
    let mut init: Option<Segment> = None;
    let mut media_count = 0;
    let mut total_media = 0;
    // The offset the next omitted-offset byte-range should start at, per URL.
    let mut next_offset: HashMap<String, u64> = HashMap::new();
    // A pending `#EXT-X-BYTERANGE` that applies to the upcoming segment URI.
    let mut pending: Option<(u64, Option<u64>)> = None;
    let mut entries = Vec::new();
    let sequence = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("#EXT-X-MEDIA-SEQUENCE:"))
        .map(|s| s.parse::<u64>())
        .transpose()
        .map_err(|_| Error::InvalidInput("Invalid HLS media sequence".into()))?
        .unwrap_or(0);
    let target = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("#EXT-X-TARGETDURATION:"))
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(4.0);
    let mut duration = None;

    for line in text.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("#EXTINF:") {
            duration = value
                .split(',')
                .next()
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v > 0.0);
        } else if line == "#EXT-X-DISCONTINUITY" {
            if media_count > 0 {
                groups.push(std::mem::take(&mut segments));
                media_count = 0;
                if let Some(init) = &init {
                    segments.push(init.clone());
                }
            }
        } else if let Some(attrs) = line.strip_prefix("#EXT-X-MAP:") {
            let Some(uri) = attr(attrs, "URI")
                .map(strip_quotes)
                .filter(|u| !u.is_empty())
            else {
                return Err(Error::InvalidInput(
                    "HLS initialization map has no URI".into(),
                ));
            };
            let url = resolve(base, &uri);
            let range = attr(attrs, "BYTERANGE")
                .map(strip_quotes)
                .map(|br| parse_byterange(&br))
                .transpose()?
                .map(|(len, off)| resolve_range(&url, len, off, &mut next_offset))
                .transpose()?;
            if range.is_none() {
                next_offset.clear();
            }
            let map = Segment { url, range };
            if init.as_ref() != Some(&map) {
                if media_count > 0 {
                    groups.push(std::mem::take(&mut segments));
                } else {
                    segments.clear();
                }
                media_count = 0;
                segments.push(map.clone());
                init = Some(map);
            }
        } else if let Some(br) = line.strip_prefix("#EXT-X-BYTERANGE:") {
            pending = Some(parse_byterange(br)?);
        } else if !line.is_empty() && !line.starts_with('#') {
            let url = resolve(base, line);
            let range = pending
                .take()
                .map(|(len, off)| resolve_range(&url, len, off, &mut next_offset))
                .transpose()?;
            if range.is_none() {
                next_offset.clear();
            }
            let segment = Segment { url, range };
            let sequence = sequence
                .checked_add(entries.len() as u64)
                .ok_or_else(|| Error::InvalidInput("HLS media sequence overflow".into()))?;
            entries.push(LiveEntry {
                sequence,
                duration: duration.take(),
                segments: init
                    .iter()
                    .cloned()
                    .chain(std::iter::once(segment.clone()))
                    .collect(),
            });
            segments.push(segment);
            media_count += 1;
            total_media += 1;
            if total_media > 100_000 || groups.len() > 255 {
                return Err(Error::InvalidInput(
                    "HLS playlist has too many segments or discontinuities".into(),
                ));
            }
        }
    }

    if pending.is_some() {
        return Err(Error::InvalidInput(
            "HLS byte range has no media URI".into(),
        ));
    }
    if media_count > 0 {
        groups.push(segments);
    }
    if groups.iter().flatten().any(|s| {
        s.range
            .is_some_and(|(offset, length)| length == 0 || offset.checked_add(length).is_none())
    }) {
        return Err(Error::InvalidInput("Invalid HLS byte range".into()));
    }
    Ok(Snapshot {
        groups,
        entries,
        target,
        ended: text.lines().any(|l| l.trim() == "#EXT-X-ENDLIST"),
    })
}

fn validate_clear_playlist(text: &str) -> Result<()> {
    for line in text.lines().map(str::trim) {
        if let Some(attrs) = line
            .strip_prefix("#EXT-X-KEY:")
            .or_else(|| line.strip_prefix("#EXT-X-SESSION-KEY:"))
        {
            if attr(attrs, "METHOD").as_deref() != Some("NONE") {
                return Err(Error::InvalidInput(
                    "Encrypted HLS (AES-128/SAMPLE-AES) is not supported".into(),
                ));
            }
        }
        if line == "#EXT-X-GAP" {
            return Err(Error::InvalidInput(
                "The HLS playlist contains missing media segments".into(),
            ));
        }
    }
    Ok(())
}

/// Parse `<length>[@<offset>]` (the `#EXT-X-BYTERANGE` / `BYTERANGE` form) into
/// `(length, Some(offset))`, or `(length, None)` when the offset is omitted.
fn parse_byterange(s: &str) -> Result<(u64, Option<u64>)> {
    let s = s.trim();
    let invalid = || Error::InvalidInput("Invalid HLS byte range".into());
    let (length, offset) = match s.split_once('@') {
        Some((len, off)) => (
            len.trim().parse::<u64>().map_err(|_| invalid())?,
            Some(off.trim().parse::<u64>().map_err(|_| invalid())?),
        ),
        None => (s.parse::<u64>().map_err(|_| invalid())?, None),
    };
    if length == 0 {
        return Err(invalid());
    }
    Ok((length, offset))
}

/// Resolve an omitted offset against the running per-URL offset and advance it,
/// yielding the concrete `(offset, length)` slice for this segment.
fn resolve_range(
    url: &str,
    length: u64,
    offset: Option<u64>,
    next_offset: &mut HashMap<String, u64>,
) -> Result<(u64, u64)> {
    let offset = offset
        .or_else(|| next_offset.get(url).copied())
        .ok_or_else(|| {
            Error::InvalidInput(
                "HLS byte range has no previous range for its omitted offset".into(),
            )
        })?;
    let end = offset
        .checked_add(length)
        .ok_or_else(|| Error::InvalidInput("HLS byte range overflows".into()))?;
    // An omitted offset refers to the immediately preceding resource only.
    next_offset.clear();
    next_offset.insert(url.to_string(), end);
    Ok((offset, length))
}

fn parse_master(text: &str, base: &str) -> Vec<VideoFormat> {
    let audio_groups = parse_audio_groups(text, base);

    let mut formats = Vec::new();
    let mut lines = text.lines().peekable();
    let mut index = 0;

    while let Some(line) = lines.next() {
        let Some(attrs) = line.strip_prefix("#EXT-X-STREAM-INF:") else {
            continue;
        };

        // The URI is the next non-comment, non-blank line.
        let uri = loop {
            match lines.peek() {
                Some(next) if next.trim().is_empty() || next.starts_with('#') => {
                    lines.next();
                }
                Some(next) => break Some(next.trim().to_string()),
                None => break None,
            }
        };
        let Some(uri) = uri else { continue };
        lines.next();

        index += 1;
        let bandwidth = attr(attrs, "BANDWIDTH").and_then(|v| v.parse::<u64>().ok());
        let (width, height) = attr(attrs, "RESOLUTION")
            .and_then(|r| r.split_once('x'))
            .and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)))
            .unzip();
        let codecs = attr(attrs, "CODECS").map(|s| s.trim_matches('"').to_string());
        let audio_only = width.is_none() && codecs.as_deref().is_some_and(|value|
            value.split(',').all(|codec| ["mp4a", "opus", "ac-3", "ec-3", "flac", "vorbis"].iter()
                .any(|prefix| codec.trim().starts_with(prefix))));
        let fps = attr(attrs, "FRAME-RATE").and_then(|v| v.parse::<f32>().ok());

        // If this variant names an audio group that carries its own URI, the
        // video rendition is video-only and the audio must be fetched and muxed
        // separately. When the group has no URI, audio is muxed into the video
        // segments already and no merge is needed.
        let audio = attr(attrs, "AUDIO")
            .map(strip_quotes)
            .and_then(|group| audio_groups.get(group.as_str()));

        let (audio_url, requires_merge) = match audio {
            Some(a) => (Some(a.url.clone()), true),
            None => (None, false),
        };

        formats.push(VideoFormat {
            validation: None,
            download_options: None,
            id: format!("hls-{index}"),
            // Both the muxed and the merged path land in an .mp4 (the download
            // step remuxes/muxes the TS segments), so advertise mp4 up front.
            container: if audio_only { "m4a" } else { "mp4" }.into(),
            video_codec: if audio_only { None } else { codecs.clone() },
            audio_codec: if audio_only { codecs } else { audio.and_then(|a| a.codec.clone()) },
            width,
            height,
            fps,
            bitrate: bandwidth,
            filesize: None,
            video_url: Some(resolve(base, &uri)),
            audio_url,
            has_audio: if audio_only { Some(true) } else { None },
            headers: Vec::new(),
            requires_merge,
        });
    }

    formats
}

/// Collect `#EXT-X-MEDIA:TYPE=AUDIO` renditions keyed by their `GROUP-ID`.
/// Only groups that carry a `URI` matter here — a group without one means the
/// audio is already muxed into the video segments. When a group lists several
/// renditions, the `DEFAULT=YES` one wins, else the first seen.
fn parse_audio_groups(text: &str, base: &str) -> HashMap<String, AudioRendition> {
    let mut groups: HashMap<String, AudioRendition> = HashMap::new();

    for line in text.lines() {
        let Some(attrs) = line.strip_prefix("#EXT-X-MEDIA:") else {
            continue;
        };
        if attr(attrs, "TYPE").map(strip_quotes).as_deref() != Some("AUDIO") {
            continue;
        }
        let Some(group_id) = attr(attrs, "GROUP-ID").map(strip_quotes) else {
            continue;
        };
        let Some(uri) = attr(attrs, "URI")
            .map(strip_quotes)
            .filter(|u| !u.is_empty())
        else {
            continue;
        };
        let default = attr(attrs, "DEFAULT")
            .map(|v| v.trim_matches('"').eq_ignore_ascii_case("YES"))
            .unwrap_or(false);

        let candidate = AudioRendition {
            url: resolve(base, &uri),
            codec: None,
            default,
        };

        match groups.get(&group_id) {
            // A previously stored default outranks a later non-default entry.
            Some(existing) if existing.default && !default => {}
            _ => {
                groups.insert(group_id, candidate);
            }
        }
    }

    groups
}

fn strip_quotes(s: &str) -> String {
    s.trim().trim_matches('"').to_string()
}

/// Read one `KEY=VALUE` (or `KEY="quoted, value"`) out of an attribute list,
/// respecting commas inside quotes (`CODECS="avc1.64001f,mp4a.40.2"`).
fn attr<'a>(attrs: &'a str, key: &str) -> Option<&'a str> {
    let mut in_quotes = false;
    let mut start = 0;
    let mut parts = Vec::new();
    for (i, c) in attrs.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                parts.push(&attrs[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&attrs[start..]);

    parts.into_iter().find_map(|p| {
        let (k, v) = p.trim().split_once('=')?;
        (k == key).then(|| v.trim())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discontinuities_reinsert_current_init_and_changed_maps_start_new_groups() {
        let groups = parse_media_groups("#EXTM3U\n#EXT-X-MAP:URI=\"init1.mp4\"\none.m4s\n#EXT-X-DISCONTINUITY\ntwo.m4s\n#EXT-X-MAP:URI=\"init2.mp4\"\nthree.m4s\n#EXT-X-ENDLIST", "https://cdn.example/").unwrap();
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0][0].url, "https://cdn.example/init1.mp4");
        assert_eq!(groups[1][0].url, "https://cdn.example/init1.mp4");
        assert_eq!(groups[2][0].url, "https://cdn.example/init2.mp4");
        assert_eq!(groups[1][1].url, "https://cdn.example/two.m4s");
    }

    #[test]
    fn repeated_map_does_not_duplicate_init_inside_a_continuous_group() {
        let groups = parse_media_groups(
            "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\na.m4s\n#EXT-X-MAP:URI=\"init.mp4\"\nb.m4s",
            "https://cdn.example/",
        )
        .unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 3);
    }

    #[test]
    fn implicit_ranges_must_follow_a_range_of_the_same_resource() {
        for playlist in [
            "#EXTM3U\n#EXT-X-BYTERANGE:10\na.mp4",
            "#EXTM3U\n#EXT-X-BYTERANGE:10@0\na.mp4\n#EXT-X-BYTERANGE:10@0\nb.mp4\n#EXT-X-BYTERANGE:10\na.mp4",
            "#EXTM3U\na.ts\n#EXT-X-BYTERANGE:10@invalid\nb.ts",
            "#EXTM3U\na.ts\n#EXT-X-BYTERANGE:10@0",
        ] {
            assert!(parse_media_groups(playlist, "https://cdn.example/").is_err());
        }
    }

    #[test]
    fn encrypted_keys_gaps_and_bad_ranges_are_rejected() {
        for tag in [
            "#EXT-X-KEY:METHOD=AES-128,URI=\"key\"",
            "#EXT-X-KEY:METHOD=SAMPLE-AES",
            "#EXT-X-GAP",
            "#EXT-X-BYTERANGE:0@0",
            "#EXT-X-BYTERANGE:2@18446744073709551615",
        ] {
            assert!(
                parse_media_groups(
                    &format!("#EXTM3U\na.ts\n{tag}\nb.ts"),
                    "https://cdn.example/"
                )
                .is_err(),
                "{tag}"
            );
        }
        assert!(
            parse_media_groups(
                "#EXTM3U\n#EXT-X-KEY:METHOD=NONE\na.ts",
                "https://cdn.example/"
            )
            .is_ok()
        );
    }

    const MASTER: &str = "#EXTM3U\n\
#EXT-X-STREAM-INF:BANDWIDTH=2500000,RESOLUTION=1280x720,CODECS=\"avc1.64001f,mp4a.40.2\"\n\
720p.m3u8\n\
#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360\n\
360p.m3u8\n";

    const MASTER_SPLIT_AUDIO: &str = "#EXTM3U\n\
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"English\",DEFAULT=YES,URI=\"audio/en.m3u8\"\n\
#EXT-X-STREAM-INF:BANDWIDTH=2500000,RESOLUTION=1280x720,CODECS=\"avc1.64001f\",AUDIO=\"aud\"\n\
video/720p.m3u8\n\
#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360,AUDIO=\"aud\"\n\
video/360p.m3u8\n";

    #[test]
    fn parses_every_rendition_in_a_master_playlist() {
        let formats = parse_master(MASTER, "https://cdn.example/video/");
        assert_eq!(formats.len(), 2);
        assert_eq!(formats[0].width, Some(1280));
        assert_eq!(formats[0].height, Some(720));
        assert_eq!(formats[0].bitrate, Some(2_500_000));
        assert_eq!(
            formats[0].video_codec.as_deref(),
            Some("avc1.64001f,mp4a.40.2")
        );
        assert_eq!(
            formats[0].video_url.as_deref(),
            Some("https://cdn.example/video/720p.m3u8")
        );
        assert_eq!(formats[0].container, "mp4");
        // Audio is muxed into the rendition here, so no separate merge.
        assert!(!formats[0].requires_merge);
        assert_eq!(formats[1].width, Some(640));
    }

    #[test]
    fn a_separate_audio_group_becomes_a_merge_with_the_right_audio_url() {
        let formats = parse_master(MASTER_SPLIT_AUDIO, "https://cdn.example/v/");
        assert_eq!(formats.len(), 2);
        for f in &formats {
            assert!(f.requires_merge, "split-audio variants must merge");
            assert_eq!(
                f.audio_url.as_deref(),
                Some("https://cdn.example/v/audio/en.m3u8")
            );
            assert_eq!(f.container, "mp4");
        }
        assert_eq!(
            formats[0].video_url.as_deref(),
            Some("https://cdn.example/v/video/720p.m3u8")
        );
    }

    #[test]
    fn attribute_parsing_ignores_commas_inside_quotes() {
        let attrs = r#"BANDWIDTH=100,CODECS="a,b",RESOLUTION=1x1"#;
        assert_eq!(attr(attrs, "CODECS"), Some(r#""a,b""#));
        assert_eq!(attr(attrs, "BANDWIDTH"), Some("100"));
    }

    #[test]
    fn media_playlist_includes_init_segment_from_map_tag() {
        let playlist = "#EXTM3U\n\
#EXT-X-MAP:URI=\"init.mp4\"\n\
#EXTINF:4.0,\n\
seg-1.m4s\n\
#EXTINF:4.0,\n\
seg-2.m4s\n";
        let segments = parse_media_segments(playlist, "https://cdn.example/video/");
        assert_eq!(
            segments,
            vec![
                Segment::whole("https://cdn.example/video/init.mp4".into()),
                Segment::whole("https://cdn.example/video/seg-1.m4s".into()),
                Segment::whole("https://cdn.example/video/seg-2.m4s".into()),
            ]
        );
    }

    #[test]
    fn byte_range_segments_slice_a_single_file() {
        // The fMP4-in-one-file shape X/Twitter serves: init + every segment are
        // byte ranges of the same clip.mp4. Omitted offsets continue from the
        // previous sub-range.
        let playlist = "#EXTM3U\n\
#EXT-X-MAP:URI=\"clip.mp4\",BYTERANGE=\"731@0\"\n\
#EXTINF:4.0,\n\
#EXT-X-BYTERANGE:245616@731\n\
clip.mp4\n\
#EXTINF:4.0,\n\
#EXT-X-BYTERANGE:198204\n\
clip.mp4\n";
        let segments = parse_media_segments(playlist, "https://video.twimg.example/v/");
        let url = "https://video.twimg.example/v/clip.mp4";
        assert_eq!(
            segments,
            vec![
                Segment {
                    url: url.into(),
                    range: Some((0, 731))
                },
                Segment {
                    url: url.into(),
                    range: Some((731, 245616))
                },
                // Offset omitted → continues at 731 + 245616 = 246347.
                Segment {
                    url: url.into(),
                    range: Some((246347, 198204))
                },
            ]
        );
    }

    #[test]
    fn byte_range_without_offset_parses_length_only() {
        assert_eq!(parse_byterange("245616@731").unwrap(), (245616, Some(731)));
        assert_eq!(parse_byterange("198204").unwrap(), (198204, None));
    }
}
