//! DASH: turn an MPD manifest into downloadable formats.
//!
//! Two shapes of representation are understood, and they are quite different
//! kinds of work:
//!
//! * `SegmentTemplate` — the stream is a numbered list of segment URLs, which
//!   this module expands and hands to [`super::stream`] as a concatenation.
//! * `SegmentBase` (or a bare `BaseURL`) — the whole track already sits at one
//!   URL and is fetched as a single file. This is the shape Facebook and
//!   Instagram serve, and supporting it is what lets their manifests be read
//!   at all.
//!
//! Either way a video representation is video *only*: DASH keeps the audio in
//! its own adaptation set. Every video rendition is therefore paired with the
//! best audio rendition and marked `requires_merge`, and a manifest with no
//! audio at all yields formats that say so (`has_audio: Some(false)`) rather
//! than quietly producing a silent file.
//!
//! Finite timelines and compatible periods are supported. Protected content
//! and dynamic manifests are rejected before downloading media.

use reqwest::Client;
use serde::Deserialize;

use super::net::{base_url, fetch_text, resolve};
use super::{VideoFormat, VideoMetadata};
use crate::error::{Error, Result};
use crate::headers::Headers;

pub async fn analyze(client: &Client, url: &str, headers: &Headers) -> Result<VideoMetadata> {
    let text = fetch_text(client, url, headers).await?;
    let formats = formats_from_manifest(&text, &base_url(url))?;
    let duration = quick_xml::de::from_str::<Mpd>(&text).ok().and_then(|mpd| {
        mpd.media_presentation_duration
            .as_deref()
            .and_then(parse_iso_duration)
            .or_else(|| {
                let durations: Option<Vec<f64>> = mpd
                    .periods
                    .iter()
                    .map(|p| p.duration.as_deref().and_then(parse_iso_duration))
                    .collect();
                durations.filter(|d| !d.is_empty()).map(|d| d.iter().sum())
            })
    });

    Ok(VideoMetadata {
        subtitles: Vec::new(),
        title: super::direct::title_from_url(url),
        thumbnail: None,
        duration,
        uploader: None,
        webpage_url: url.to_string(),
        formats,
    })
}

/// Parse manifest XML that is already in hand into formats.
///
/// Separate from [`analyze`] because Facebook and Instagram do not link their
/// manifests — they inline the XML in the page's JSON, so there is nothing to
/// fetch and no manifest URL to resolve against. `base` is then the page's own
/// base, which those manifests never need because every `BaseURL` in them is
/// absolute.
pub fn formats_from_manifest(xml: &str, base: &str) -> Result<Vec<VideoFormat>> {
    let mpd: Mpd = quick_xml::de::from_str(xml)
        .map_err(|e| Error::InvalidInput(format!("could not parse DASH manifest: {e}")))?;

    if mpd.kind.as_deref() == Some("dynamic") {
        return Err(Error::InvalidInput(
            "Live DASH recording is not supported yet".into(),
        ));
    }
    if mpd.periods.len() > 256 {
        return Err(Error::InvalidInput(
            "DASH manifest has too many periods".into(),
        ));
    }
    let root_base = match &mpd.base_url {
        Some(b) => resolve(base, b),
        None => base.to_string(),
    };
    let total_secs = mpd
        .media_presentation_duration
        .as_deref()
        .and_then(parse_iso_duration);

    let mut periods = Vec::new();
    let mut elapsed = 0.0;
    for (index, period) in mpd.periods.iter().enumerate() {
        let start = period
            .start
            .as_deref()
            .and_then(parse_iso_duration)
            .unwrap_or(elapsed);
        if (start - elapsed).abs() > 0.05 {
            return Err(Error::InvalidInput(
                "DASH periods contain a gap or overlap".into(),
            ));
        }
        let period_secs = period
            .duration
            .as_deref()
            .and_then(parse_iso_duration)
            .or_else(|| {
                mpd.periods
                    .get(index + 1)
                    .and_then(|p| p.start.as_deref())
                    .and_then(parse_iso_duration)
                    .map(|end| end - start)
            })
            .or_else(|| {
                if index + 1 == mpd.periods.len() {
                    total_secs.map(|end| end - start)
                } else {
                    None
                }
            });
        if mpd.periods.len() > 1 && !period_secs.is_some_and(|d| d > 0.0) {
            return Err(Error::InvalidInput(
                "DASH period duration is not bounded".into(),
            ));
        }
        elapsed = start + period_secs.unwrap_or(0.0);
        let mut video_candidates: Vec<(Representation, String)> = Vec::new();
        let mut audio_candidates: Vec<(Representation, String)> = Vec::new();
        let period_base = match &period.base_url {
            Some(b) => resolve(&root_base, b),
            None => root_base.clone(),
        };

        for set in &period.adaptation_sets {
            let set_base = match &set.base_url {
                Some(b) => resolve(&period_base, b),
                None => period_base.clone(),
            };

            if !set.protection.is_empty() {
                return Err(Error::InvalidInput(
                    "Protected DASH content is not supported".into(),
                ));
            }
            for rep in &set.representations {
                if !rep.protection.is_empty() {
                    return Err(Error::InvalidInput(
                        "Protected DASH content is not supported".into(),
                    ));
                }
                let mut rep = rep.clone();
                rep.codecs = rep.codecs.or_else(|| set.codecs.clone());
                rep.width = rep.width.or(set.width);
                rep.height = rep.height.or(set.height);
                rep.frame_rate = rep.frame_rate.or_else(|| set.frame_rate.clone());
                rep.mime_type = rep.mime_type.or_else(|| set.mime_type.clone());
                let rep = &rep;
                // The media type can sit on either the adaptation set or the
                // representation, and DASH allows `contentType` in place of
                // `mimeType`. Meta's manifests use every one of these spellings.
                let kind = rep
                    .mime_type
                    .as_deref()
                    .or(set.mime_type.as_deref().or(set.content_type.as_deref()))
                    .unwrap_or("");
                let is_video = kind.starts_with("video");
                let is_audio = kind.starts_with("audio");
                if !is_video && !is_audio {
                    continue;
                }

                let Some(source) = representation_source(&set_base, period, set, rep, period_secs)
                else {
                    continue;
                };

                if is_video {
                    video_candidates.push((rep.clone(), source));
                } else {
                    audio_candidates.push((rep.clone(), source));
                }
            }
        }

        if video_candidates.is_empty() {
            if audio_candidates.is_empty() {
                return Err(Error::InvalidInput("no usable media representations in DASH manifest".into()));
            }
            let mut formats: Vec<VideoFormat> = audio_candidates.into_iter().map(|(rep, source)| VideoFormat {
                id: format!("dash-audio-{}", rep.id), container: if rep.mime_type.as_deref().is_some_and(|mime| mime.contains("webm")) { "webm" } else { "m4a" }.into(),
                audio_codec: rep.codecs, bitrate: rep.bandwidth, video_url: Some(source),
                has_audio: Some(true), ..Default::default()
            }).collect();
            super::sort_formats(&mut formats);
            periods.push(formats);
            continue;
        }

        // Every video candidate is paired with the single best (highest
        // bandwidth) audio candidate, since a manifest with separate audio
        // adaptation sets rarely varies audio quality per video rendition.
        let best_audio = audio_candidates
            .iter()
            .max_by_key(|(rep, _)| rep.bandwidth.unwrap_or(0));

        let mut formats: Vec<VideoFormat> = video_candidates
            .into_iter()
            .map(|(rep, video_source)| {
                let audio_url = best_audio.map(|(_, src)| src.clone());
                VideoFormat {
                    validation: None,
                    download_options: None,
                    id: format!("dash-{}", rep.id),
                    container: "mp4".into(),
                    video_codec: rep.codecs.clone(),
                    audio_codec: best_audio.and_then(|(r, _)| r.codecs.clone()),
                    width: rep.width,
                    height: rep.height,
                    fps: rep.frame_rate.as_deref().and_then(parse_frame_rate),
                    bitrate: rep.bandwidth,
                    filesize: None,
                    video_url: Some(video_source),
                    requires_merge: audio_url.is_some(),
                    audio_url,
                    // A DASH video representation never carries its own audio. Say
                    // so plainly when there was no audio track to pair it with —
                    // that is the difference between the picker offering a silent
                    // download and the picker admitting that is what it is.
                    has_audio: Some(false),
                    headers: Vec::new(),
                }
            })
            .collect();

        super::sort_formats(&mut formats);
        periods.push(formats);
    }
    if total_secs.is_some_and(|total| (elapsed - total).abs() > 0.05) {
        return Err(Error::InvalidInput(
            "DASH periods do not cover the declared presentation duration".into(),
        ));
    }
    let mut formats = periods.first().cloned().unwrap_or_default();
    if periods.len() > 1 {
        formats.retain_mut(|format| {
            let mut video = Vec::new();
            let mut audio = Vec::new();
            for period in &periods {
                let matching: Vec<_> = period
                    .iter()
                    .filter(|f| {
                        f.width == format.width
                            && f.height == format.height
                            && f.video_codec == format.video_codec
                            && f.audio_codec == format.audio_codec
                            && f.requires_merge == format.requires_merge
                    })
                    .collect();
                let chosen = matching
                    .iter()
                    .find(|f| f.id == format.id)
                    .copied()
                    .or_else(|| {
                        if matching.len() == 1 {
                            Some(matching[0])
                        } else {
                            None
                        }
                    });
                let Some(chosen) = chosen else { return false };
                let Some(v) = chosen.video_url.as_deref().and_then(source_segments) else {
                    return false;
                };
                video.push(v);
                if format.requires_merge {
                    let Some(a) = chosen.audio_url.as_deref().and_then(source_segments) else {
                        return false;
                    };
                    audio.push(a);
                }
            }
            format.video_url = Some(super::groups::encode(&video));
            if format.requires_merge {
                format.audio_url = Some(super::groups::encode(&audio));
            }
            true
        });
    }
    if formats.is_empty() {
        return Err(Error::InvalidInput(
            "No compatible video/audio renditions across all DASH periods".into(),
        ));
    }
    Ok(formats)
}

/// A single fetchable source for one representation, or `None` when its shape
/// is one this module does not handle.
///
/// The `SegmentTemplate` case expands to a segment list; everything else falls
/// back to the representation's own `BaseURL`, which for `SegmentBase` content
/// is the complete track in one file.
fn representation_source(
    set_base: &str,
    period: &Period,
    set: &AdaptationSet,
    rep: &Representation,
    total_secs: Option<f64>,
) -> Option<String> {
    let base = match &rep.base_url {
        Some(b) => resolve(set_base, b),
        None => set_base.to_string(),
    };

    let template = inherit_template(
        period.segment_template.as_ref(),
        set.segment_template.as_ref(),
    );
    if let Some(template) = inherit_template(template.as_ref(), rep.segment_template.as_ref()) {
        // A failed template never falls back to downloading its directory.
        let segments = build_segment_urls(&base, &template, &rep.id, rep.bandwidth, total_secs)?;
        return Some(super::stream::encode_segments(&segments));
    }
    if rep
        .segment_base
        .as_ref()
        .or(set.segment_base.as_ref())
        .is_some_and(|segment| segment.offset.unwrap_or(0) != 0)
    {
        // Whole-file download cannot safely trim a period starting mid-track.
        return None;
    }

    // No template (or one we could not expand): the representation's BaseURL is
    // the stream itself. It has to actually name a file — a `BaseURL` of "./"
    // is a path prefix for segments, and handing back the directory would start
    // a download of whatever index page sits there.
    let names_a_file = rep.base_url.is_some() || set.base_url.is_some();
    if names_a_file && base.starts_with("http") && !base.ends_with('/') {
        return Some(base);
    }
    None
}

fn source_segments(source: &str) -> Option<Vec<super::stream::Segment>> {
    if source.starts_with("http://") || source.starts_with("https://") {
        return Some(vec![super::stream::Segment::whole(source.to_string())]);
    }
    let urls: Vec<String> =
        serde_json::from_str(source.strip_prefix("zoltraak-dash-segments:")?).ok()?;
    Some(
        urls.into_iter()
            .map(super::stream::Segment::whole)
            .collect(),
    )
}

fn inherit_template(
    parent: Option<&SegmentTemplate>,
    child: Option<&SegmentTemplate>,
) -> Option<SegmentTemplate> {
    let Some(child) = child else {
        return parent.cloned();
    };
    let mut result = child.clone();
    if let Some(parent) = parent {
        if result.media.is_empty() {
            result.media = parent.media.clone();
        }
        result.initialization = result
            .initialization
            .or_else(|| parent.initialization.clone());
        result.start_number = result.start_number.or(parent.start_number);
        result.duration = result.duration.or(parent.duration);
        result.timescale = result.timescale.or(parent.timescale);
        result.offset = result.offset.or(parent.offset);
        result.ept_delta = result.ept_delta.or(parent.ept_delta);
        result.timeline = result.timeline.or_else(|| parent.timeline.clone());
    }
    Some(result)
}

const MAX_SEGMENTS: usize = 100_000;
fn build_segment_urls(
    base: &str,
    template: &SegmentTemplate,
    rep_id: &str,
    bandwidth: Option<u64>,
    total_secs: Option<f64>,
) -> Option<Vec<String>> {
    let scale = template.timescale.unwrap_or(1);
    if scale == 0 || template.media.is_empty() || template.ept_delta.unwrap_or(0) != 0 {
        return None;
    }
    let offset = template.offset.unwrap_or(0);
    let end = total_secs
        .filter(|d| d.is_finite() && *d > 0.0)
        .map(|d| offset as f64 + d * scale as f64);
    let mut times = Vec::new();
    if let Some(timeline) = &template.timeline {
        let mut time = 0u64;
        for (i, entry) in timeline.entries.iter().enumerate() {
            if entry.duration == 0 || entry.repeat < -1 {
                return None;
            }
            let next = entry.time.unwrap_or(time);
            if times.is_empty() && next != offset {
                // Non-aligned first segments need period trimming.
                return None;
            }
            if !times.is_empty() && next != time {
                return None;
            }
            time = next;
            let count = if entry.repeat >= 0 {
                (entry.repeat as u64).checked_add(1)?
            } else {
                let bound = timeline
                    .entries
                    .get(i + 1)
                    .and_then(|s| s.time)
                    .map(|t| t as f64)
                    .or(end)?;
                if bound <= time as f64 {
                    return None;
                }
                ((bound - time as f64) / entry.duration as f64).ceil() as u64
            };
            if count == 0
                || count > MAX_SEGMENTS as u64
                || times.len().checked_add(count as usize)? > MAX_SEGMENTS
            {
                return None;
            }
            for _ in 0..count {
                if end.is_none_or(|end| (time as f64) < end) {
                    times.push(time);
                }
                time = time.checked_add(entry.duration)?;
            }
        }
    } else {
        let duration = template.duration?;
        if duration == 0 {
            return None;
        }
        let count = ((end? - offset as f64) / duration as f64).ceil();
        if !count.is_finite() || count < 1.0 || count > MAX_SEGMENTS as f64 {
            return None;
        }
        for i in 0..count as u64 {
            times.push(offset.checked_add(i.checked_mul(duration)?)?);
        }
    }
    if times.is_empty() {
        return None;
    }
    let start = template.start_number.unwrap_or(1);
    let mut urls = Vec::new();
    if let Some(init) = &template.initialization {
        urls.push(resolve(
            base,
            &expand_template(init, rep_id, bandwidth, start, times[0])?,
        ));
    }
    for (i, time) in times.into_iter().enumerate() {
        urls.push(resolve(
            base,
            &expand_template(
                &template.media,
                rep_id,
                bandwidth,
                start.checked_add(i as u64)?,
                time,
            )?,
        ));
    }
    Some(urls)
}

fn expand_template(
    template: &str,
    id: &str,
    bandwidth: Option<u64>,
    number: u64,
    time: u64,
) -> Option<String> {
    let mut output = String::new();
    let mut rest = template;
    while let Some(index) = rest.find('$') {
        output.push_str(&rest[..index]);
        rest = &rest[index + 1..];
        if let Some(tail) = rest.strip_prefix('$') {
            output.push('$');
            rest = tail;
            continue;
        }
        let end = rest.find('$')?;
        let token = &rest[..end];
        rest = &rest[end + 1..];
        if token == "RepresentationID" {
            output.push_str(id);
            continue;
        }
        let (name, width) = if let Some((name, spec)) = token.split_once('%') {
            let width: usize = spec.strip_prefix('0')?.strip_suffix('d')?.parse().ok()?;
            if width > 20 {
                return None;
            }
            (name, width)
        } else {
            (token, 0)
        };
        let value = match name {
            "Number" => number,
            "Time" => time,
            "Bandwidth" => bandwidth?,
            _ => return None,
        };
        output.push_str(&format!("{value:0width$}"));
    }
    output.push_str(rest);
    Some(output)
}

pub(crate) fn parse_iso_duration(s: &str) -> Option<f64> {
    let s = s.strip_prefix("PT")?;
    let mut total = 0.0;
    let mut num = String::new();
    for c in s.chars() {
        match c {
            '0'..='9' | '.' => num.push(c),
            'H' => {
                total += num.parse::<f64>().ok()? * 3600.0;
                num.clear();
            }
            'M' => {
                total += num.parse::<f64>().ok()? * 60.0;
                num.clear();
            }
            'S' => {
                total += num.parse::<f64>().ok()?;
                num.clear();
            }
            _ => return None,
        }
    }
    if !num.is_empty() || !total.is_finite() || s.is_empty() {
        return None;
    }
    Some(total)
}

fn parse_frame_rate(s: &str) -> Option<f32> {
    let rate = match s.split_once('/') {
        Some((n, d)) => Some(n.parse::<f32>().ok()? / d.parse::<f32>().ok()?),
        None => s.parse().ok(),
    }?;
    if rate.is_finite() && rate > 0.0 {
        Some(rate)
    } else {
        None
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename = "MPD")]
struct Mpd {
    #[serde(rename = "@type", default)]
    kind: Option<String>,
    #[serde(rename = "@mediaPresentationDuration", default)]
    media_presentation_duration: Option<String>,
    #[serde(rename = "BaseURL", default)]
    base_url: Option<String>,
    #[serde(rename = "Period", default)]
    periods: Vec<Period>,
}

#[derive(Debug, Deserialize)]
struct Period {
    #[serde(rename = "@start", default)]
    start: Option<String>,
    #[serde(rename = "@duration", default)]
    duration: Option<String>,
    #[serde(rename = "SegmentTemplate", default)]
    segment_template: Option<SegmentTemplate>,
    #[serde(rename = "BaseURL", default)]
    base_url: Option<String>,
    #[serde(rename = "AdaptationSet", default)]
    adaptation_sets: Vec<AdaptationSet>,
}

#[derive(Debug, Deserialize)]
struct AdaptationSet {
    #[serde(rename = "SegmentBase", default)]
    segment_base: Option<SegmentBase>,
    #[serde(rename = "ContentProtection", default)]
    protection: Vec<Protection>,
    #[serde(rename = "@codecs", default)]
    codecs: Option<String>,
    #[serde(rename = "@width", default)]
    width: Option<u32>,
    #[serde(rename = "@height", default)]
    height: Option<u32>,
    #[serde(rename = "@frameRate", default)]
    frame_rate: Option<String>,

    #[serde(rename = "@mimeType", default)]
    mime_type: Option<String>,
    #[serde(rename = "@contentType", default)]
    content_type: Option<String>,
    #[serde(rename = "BaseURL", default)]
    base_url: Option<String>,
    #[serde(rename = "SegmentTemplate", default)]
    segment_template: Option<SegmentTemplate>,
    #[serde(rename = "Representation", default)]
    representations: Vec<Representation>,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct SegmentTemplate {
    #[serde(rename = "@eptDelta", default)]
    ept_delta: Option<i64>,
    #[serde(rename = "@media", default)]
    media: String,
    #[serde(rename = "@initialization", default)]
    initialization: Option<String>,
    #[serde(rename = "@startNumber", default)]
    start_number: Option<u64>,
    #[serde(rename = "@duration", default)]
    duration: Option<u64>,
    #[serde(rename = "@timescale", default)]
    timescale: Option<u64>,
    #[serde(rename = "@presentationTimeOffset", default)]
    offset: Option<u64>,
    #[serde(rename = "SegmentTimeline", default)]
    timeline: Option<Timeline>,
}

#[derive(Debug, Deserialize, Clone)]
struct Protection {}
#[derive(Debug, Deserialize, Clone)]
struct SegmentBase {
    #[serde(rename = "@presentationTimeOffset", default)]
    offset: Option<u64>,
}
#[derive(Debug, Deserialize, Clone)]
struct Timeline {
    #[serde(rename = "S", default)]
    entries: Vec<TimelineEntry>,
}
#[derive(Debug, Deserialize, Clone)]
struct TimelineEntry {
    #[serde(rename = "@t", default)]
    time: Option<u64>,
    #[serde(rename = "@d")]
    duration: u64,
    #[serde(rename = "@r", default)]
    repeat: i64,
}

#[derive(Debug, Deserialize, Clone)]
struct Representation {
    #[serde(rename = "SegmentBase", default)]
    segment_base: Option<SegmentBase>,
    #[serde(rename = "ContentProtection", default)]
    protection: Vec<Protection>,

    #[serde(rename = "@id")]
    id: String,
    #[serde(rename = "@mimeType", default)]
    mime_type: Option<String>,
    #[serde(rename = "BaseURL", default)]
    base_url: Option<String>,
    #[serde(rename = "@bandwidth", default)]
    bandwidth: Option<u64>,
    #[serde(rename = "@width", default)]
    width: Option<u32>,
    #[serde(rename = "@height", default)]
    height: Option<u32>,
    #[serde(rename = "@codecs", default)]
    codecs: Option<String>,
    #[serde(rename = "@frameRate", default)]
    frame_rate: Option<String>,
    #[serde(rename = "SegmentTemplate", default)]
    segment_template: Option<SegmentTemplate>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variable_timeline_inherits_template_and_formats_time_and_number() {
        let xml = r#"<MPD mediaPresentationDuration="PT7S"><Period>
        <SegmentTemplate timescale="10" startNumber="7" initialization="init-$RepresentationID$-$Bandwidth$.mp4"/>
        <AdaptationSet contentType="video" codecs="avc1" width="640" height="360">
        <SegmentTemplate media="seg-$Number%03d$-$Time$-$$.m4s"><SegmentTimeline><S t="0" d="20" r="1"/><S d="30"/></SegmentTimeline></SegmentTemplate>
        <Representation id="v" bandwidth="123"/></AdaptationSet></Period></MPD>"#;
        let f = formats_from_manifest(xml, "https://cdn.example/")
            .unwrap()
            .remove(0);
        let segments = source_segments(f.video_url.as_deref().unwrap()).unwrap();
        assert_eq!(f.height, Some(360));
        assert_eq!(f.video_codec.as_deref(), Some("avc1"));
        assert_eq!(
            segments.iter().map(|s| s.url.as_str()).collect::<Vec<_>>(),
            vec![
                "https://cdn.example/init-v-123.mp4",
                "https://cdn.example/seg-007-0-$.m4s",
                "https://cdn.example/seg-008-20-$.m4s",
                "https://cdn.example/seg-009-40-$.m4s"
            ]
        );
    }

    #[test]
    fn negative_repeat_uses_next_start_or_period_end_and_offset() {
        let template: SegmentTemplate = quick_xml::de::from_str(r#"<SegmentTemplate media="$Time$.m4s" timescale="10" presentationTimeOffset="100">
        <SegmentTimeline><S t="100" d="20" r="-1"/><S t="140" d="30" r="-1"/></SegmentTimeline></SegmentTemplate>"#).unwrap();
        let urls =
            build_segment_urls("https://cdn.example/", &template, "v", None, Some(10.0)).unwrap();
        assert_eq!(
            urls,
            vec![
                "https://cdn.example/100.m4s",
                "https://cdn.example/120.m4s",
                "https://cdn.example/140.m4s",
                "https://cdn.example/170.m4s"
            ]
        );
        assert!(build_segment_urls("https://cdn.example/", &template, "v", None, None).is_none());
    }

    #[test]
    fn malformed_or_excessive_timelines_cannot_expand_or_fall_back_to_file() {
        for entries in [
            r#"<S d="0"/>"#,
            r#"<S d="1" r="100000"/>"#,
            r#"<S d="1" r="-2"/>"#,
            r#"<S t="18446744073709551615" d="2"/>"#,
            r#"<S d="2"/><S t="1" d="2"/>"#,
        ] {
            let xml = format!(
                r#"<MPD><Period><AdaptationSet contentType="video"><Representation id="v"><BaseURL>file.mp4</BaseURL><SegmentTemplate media="$Time$.m4s"><SegmentTimeline>{entries}</SegmentTimeline></SegmentTemplate></Representation></AdaptationSet></Period></MPD>"#
            );
            assert!(
                formats_from_manifest(&xml, "https://cdn.example/").is_err(),
                "{entries}"
            );
        }
        assert!(expand_template("$Number%099999d$", "v", None, 1, 0).is_none());
        assert!(expand_template("$Unknown$", "v", None, 1, 0).is_none());
    }

    fn two_periods() -> String {
        let period = |index| {
            format!(
                r#"<Period start="PT{index}S" duration="PT1S"><AdaptationSet contentType="video"><Representation id="v" width="64" height="64" codecs="avc1"><BaseURL>p{index}-v.mp4</BaseURL></Representation></AdaptationSet><AdaptationSet contentType="audio"><Representation id="a" codecs="mp4a"><BaseURL>p{index}-a.mp4</BaseURL></Representation></AdaptationSet></Period>"#
            )
        };
        format!(
            r#"<MPD mediaPresentationDuration="PT2S">{}{}</MPD>"#,
            period(0),
            period(1)
        )
    }

    #[test]
    fn offsets_requiring_period_trimming_are_not_ignored() {
        let xml = two_periods().replace(
            "<BaseURL>p0-v.mp4</BaseURL>",
            "<BaseURL>p0-v.mp4</BaseURL><SegmentBase presentationTimeOffset=\"10\"/>",
        );
        assert!(formats_from_manifest(&xml, "https://cdn.example/").is_err());
        for attributes in ["presentationTimeOffset=\"10\"", "eptDelta=\"-10\""] {
            let template: SegmentTemplate = quick_xml::de::from_str(&format!(r#"<SegmentTemplate media="$Time$.m4s" {attributes}><SegmentTimeline><S t="0" d="20"/></SegmentTimeline></SegmentTemplate>"#)).unwrap();
            assert!(
                build_segment_urls("https://cdn.example/", &template, "v", None, Some(2.0))
                    .is_none()
            );
        }
    }

    #[test]
    fn multiple_periods_keep_their_own_audio_and_video_in_order() {
        let f = formats_from_manifest(&two_periods(), "https://cdn.example/")
            .unwrap()
            .remove(0);
        let decode = |source: &str| {
            serde_json::from_str::<Vec<Vec<super::super::stream::Segment>>>(
                source.strip_prefix(super::super::groups::PREFIX).unwrap(),
            )
            .unwrap()
        };
        let video = decode(f.video_url.as_deref().unwrap());
        let audio = decode(f.audio_url.as_deref().unwrap());
        assert_eq!(video[0][0].url, "https://cdn.example/p0-v.mp4");
        assert_eq!(video[1][0].url, "https://cdn.example/p1-v.mp4");
        assert_eq!(audio[0][0].url, "https://cdn.example/p0-a.mp4");
        assert_eq!(audio[1][0].url, "https://cdn.example/p1-a.mp4");
    }

    #[test]
    fn incompatible_periods_gaps_dynamic_and_protected_content_are_rejected() {
        for xml in [two_periods().replacen("height=\"64\"", "height=\"128\"", 1),
            two_periods().replace("start=\"PT1S\"", "start=\"PT3S\""),
            two_periods().replace("mediaPresentationDuration=\"PT2S\"", "mediaPresentationDuration=\"PT5S\""),
            two_periods().replace("<MPD ", "<MPD type=\"dynamic\" "),
            two_periods().replacen("<Representation", "<ContentProtection schemeIdUri=\"urn:mpeg:dash:mp4protection:2011\"/><Representation", 1)] {
            assert!(formats_from_manifest(&xml, "https://cdn.example/").is_err());
        }
    }

    const MPD: &str = r#"<?xml version="1.0"?>
<MPD mediaPresentationDuration="PT8S">
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <Representation id="v0" bandwidth="1000000" width="1920" height="1080" codecs="avc1.64001f">
        <SegmentTemplate media="chunk-$Number$.m4s" initialization="init-$RepresentationID$.mp4" startNumber="1" duration="4" timescale="1"/>
      </Representation>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4">
      <Representation id="a0" bandwidth="128000" codecs="mp4a.40.2">
        <SegmentTemplate media="audio-$Number$.m4s" initialization="init-$RepresentationID$.mp4" startNumber="1" duration="4" timescale="1"/>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

    /// The shape Facebook and Instagram inline in their page JSON: one whole
    /// file per representation, addressed by `BaseURL`, with `SegmentBase`
    /// byte ranges the player uses for seeking and we do not need.
    const MPD_SEGMENT_BASE: &str = r#"<?xml version="1.0"?>
<MPD mediaPresentationDuration="PT30S">
  <Period>
    <AdaptationSet contentType="video">
      <Representation id="v1" mimeType="video/mp4" bandwidth="2000000" width="1280" height="720" codecs="avc1.4d401f" FBQualityLabel="720p">
        <BaseURL>https://video.xx.fbcdn.net/v/t2/f2/m69/hd.mp4?oh=aa&amp;oe=bb</BaseURL>
        <SegmentBase indexRange="881-1200"><Initialization range="0-880"/></SegmentBase>
      </Representation>
      <Representation id="v0" mimeType="video/mp4" bandwidth="600000" width="640" height="360" codecs="avc1.4d401e">
        <BaseURL>https://video.xx.fbcdn.net/v/t2/f2/m69/sd.mp4</BaseURL>
        <SegmentBase indexRange="881-1200"><Initialization range="0-880"/></SegmentBase>
      </Representation>
    </AdaptationSet>
    <AdaptationSet contentType="audio">
      <Representation id="a0" mimeType="audio/mp4" bandwidth="128000" codecs="mp4a.40.5">
        <BaseURL>https://video.xx.fbcdn.net/v/t2/f2/m69/audio.mp4</BaseURL>
        <SegmentBase indexRange="700-900"><Initialization range="0-699"/></SegmentBase>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

    #[test]
    fn segment_base_representations_become_whole_file_merge_formats() {
        let formats = formats_from_manifest(MPD_SEGMENT_BASE, "https://www.facebook.com/").unwrap();

        assert_eq!(
            formats.len(),
            2,
            "one format per video representation, audio is not a format"
        );
        // Sorted best-first, and every one carries the audio it needs.
        assert_eq!(formats[0].height, Some(720));
        assert_eq!(formats[1].height, Some(360));
        for f in &formats {
            assert!(
                f.requires_merge,
                "a DASH video track has no audio of its own"
            );
            assert_eq!(
                f.audio_url.as_deref(),
                Some("https://video.xx.fbcdn.net/v/t2/f2/m69/audio.mp4"),
                "paired with the single audio representation",
            );
            assert_eq!(f.audio_codec.as_deref(), Some("mp4a.40.5"));
            // The video URL is the file itself, not an encoded segment list.
            assert!(f.video_url.as_deref().unwrap().starts_with("https://"));
            assert!(f.plays_with_audio());
        }
        // XML entities in the signed URL survive parsing — an `&oe=` that came
        // back as `&amp;oe=` would 403 at the CDN.
        assert_eq!(
            formats[0].video_url.as_deref(),
            Some("https://video.xx.fbcdn.net/v/t2/f2/m69/hd.mp4?oh=aa&oe=bb"),
        );
    }

    #[test]
    fn a_video_only_manifest_reports_that_it_has_no_audio() {
        let xml = MPD_SEGMENT_BASE
            .split("<AdaptationSet contentType=\"audio\">")
            .next()
            .unwrap()
            .to_string()
            + "</Period></MPD>";
        let formats = formats_from_manifest(&xml, "https://www.facebook.com/").unwrap();

        assert!(!formats.is_empty());
        for f in &formats {
            assert!(!f.requires_merge);
            assert_eq!(f.has_audio, Some(false));
            // Nothing hides the fact that picking this one gives a silent file.
            assert!(!f.plays_with_audio());
        }
    }

    #[test]
    fn a_base_url_that_is_only_a_path_prefix_is_not_treated_as_a_file() {
        // A directory BaseURL with no SegmentTemplate to expand is not a
        // downloadable stream; returning it would fetch an index page.
        let xml = r#"<?xml version="1.0"?>
<MPD mediaPresentationDuration="PT30S">
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <Representation id="v0" bandwidth="1000" width="640" height="360">
        <BaseURL>https://cdn.example/chunks/</BaseURL>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;
        assert!(formats_from_manifest(xml, "https://cdn.example/").is_err());
    }

    #[test]
    fn iso_duration_parses_hours_minutes_seconds() {
        assert_eq!(parse_iso_duration("PT596.48S"), Some(596.48));
        assert_eq!(parse_iso_duration("PT1H2M3S"), Some(3723.0));
    }

    #[test]
    fn deserializes_a_minimal_mpd() {
        let mpd: Mpd = quick_xml::de::from_str(MPD).unwrap();
        assert_eq!(mpd.media_presentation_duration.as_deref(), Some("PT8S"));
        assert_eq!(mpd.periods.len(), 1);
        assert_eq!(mpd.periods[0].adaptation_sets.len(), 2);
    }

    #[test]
    fn segment_template_expands_to_two_four_second_chunks_plus_init() {
        let template = SegmentTemplate {
            media: "chunk-$Number$.m4s".into(),
            initialization: Some("init-$RepresentationID$.mp4".into()),
            start_number: Some(1),
            duration: Some(4),
            timescale: Some(1),
            ..Default::default()
        };
        let urls =
            build_segment_urls("https://cdn.example/", &template, "v0", None, Some(8.0)).unwrap();
        assert_eq!(
            urls,
            vec![
                "https://cdn.example/init-v0.mp4",
                "https://cdn.example/chunk-1.m4s",
                "https://cdn.example/chunk-2.m4s",
            ]
        );
    }
}
