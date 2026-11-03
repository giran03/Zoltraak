//! Read the selected TikTok item from web, embed and mobile payloads.
//! Shared by Video and Images; unrelated feed items never become renditions.
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

use super::{VideoFormat, VideoMetadata, direct, site};
use crate::headers::Headers;

static SCRIPTS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<script\b([^>]*)>(.*?)</script>"#).unwrap());
static STATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:window\.)?__(?:INITIAL_STATE|DEFAULT_SCOPE)__\s*=\s*").unwrap()
});

pub(crate) fn item<'a>(value: &'a Value, expected: Option<&str>) -> Option<&'a Value> {
    let mut pending = vec![(value, None, 0)];
    let mut unidentified = Vec::new();
    let mut identified = false;
    let mut visited = 0;
    while let Some((value, map_id, depth)) = pending.pop() {
        visited += 1;
        if visited > 20_000 || depth > 32 {
            break;
        }
        match value {
            Value::Object(obj) => {
                if obj.get("video").is_some_and(Value::is_object) {
                    let id = obj
                        .get("id")
                        .or_else(|| obj.get("aweme_id"))
                        .or_else(|| obj.get("itemId"))
                        .map(|v| {
                            v.as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| v.to_string())
                        })
                        .or_else(|| map_id.map(str::to_owned));
                    if let Some(id) = id {
                        identified = true;
                        if expected == Some(id.as_str()) {
                            return Some(value);
                        }
                        if expected.is_none() {
                            unidentified.push(value);
                        }
                    } else {
                        unidentified.push(value);
                    }
                }
                for (key, child) in obj {
                    let map_id = (key.len() >= 10 && key.chars().all(|c| c.is_ascii_digit()))
                        .then_some(key.as_str());
                    pending.push((child, map_id, depth + 1));
                }
            }
            Value::Array(values) => {
                for child in values {
                    pending.push((child, None, depth + 1));
                }
            }
            _ => {}
        }
    }
    if (expected.is_none() || !identified) && unidentified.len() == 1 {
        unidentified.pop()
    } else {
        None
    }
}

fn address_urls(value: &Value, out: &mut Vec<String>) {
    if out.len() >= 64 {
        return;
    }
    match value {
        Value::String(value) => {
            if value.len() > 8192 {
                return;
            }
            let value = direct::unescape_url(value);
            if let Ok(url) = reqwest::Url::parse(&value) {
                if matches!(url.scheme(), "http" | "https") && !out.contains(&value) {
                    out.push(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                address_urls(value, out);
            }
        }
        Value::Object(obj) => {
            for key in ["url", "Url", "uri", "urlList", "UrlList", "url_list"] {
                if let Some(value) = obj.get(key) {
                    address_urls(value, out);
                }
            }
        }
        _ => {}
    }
}

pub(crate) fn video_urls(video: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for key in [
        "playAddr",
        "downloadAddr",
        "play_addr",
        "download_addr",
        "play_url",
        "videoUrl",
        "play_addr_h264",
        "play_addr_bytevc1",
        "playAddrBytevc1",
        "play_addr_v2",
        "PlayAddrStruct",
        "playAddrStruct",
        "main_url",
        "fallback_url",
    ] {
        if let Some(value) = video.get(key) {
            address_urls(value, &mut out);
        }
    }
    for key in ["bitrateInfo", "bitrate_info", "bit_rate", "bitrate"] {
        if let Some(values) = video.get(key).and_then(Value::as_array) {
            for value in values {
                for key in [
                    "PlayAddr",
                    "playAddr",
                    "play_addr",
                    "downloadAddr",
                    "download_addr",
                ] {
                    if let Some(value) = value.get(key) {
                        address_urls(value, &mut out);
                    }
                }
            }
        }
    }
    out.retain(|url| {
        !direct::is_image_url(url)
            && !["mp3", "m4a", "aac"].iter().any(|ext| {
                url.split(['?', '#'])
                    .next()
                    .unwrap_or(url)
                    .ends_with(&format!(".{ext}"))
            })
    });
    out
}

pub(crate) fn metadata(
    value: &Value,
    expected: Option<&str>,
    url: &str,
    headers: &Headers,
) -> Option<VideoMetadata> {
    let item = item(value, expected)?;
    let video = &item["video"];
    let urls = video_urls(video);
    if urls.is_empty() {
        return None;
    }
    let number = |key: &str| {
        video
            .get(key)
            .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
    };
    let mut stream_headers: Headers = headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("user-agent") || k.eq_ignore_ascii_case("referer"))
        .cloned()
        .collect();
    if !stream_headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("referer"))
    {
        stream_headers.push(("Referer".into(), "https://www.tiktok.com/".into()));
    }
    let formats = urls
        .into_iter()
        .enumerate()
        .map(|(i, url)| {
            let mut format = VideoFormat {
                id: format!("tiktok-{i}"),
                video_url: Some(url.clone()),
                width: number("width").and_then(|n| n.try_into().ok()),
                height: number("height").and_then(|n| n.try_into().ok()),
                headers: stream_headers.clone(),
                ..Default::default()
            };
            let mut renditions = Vec::new();
            for key in ["bitrateInfo", "bitrate_info", "bit_rate", "bitrate"] {
                if let Some(values) = video.get(key).and_then(Value::as_array) {
                    renditions.extend(values);
                }
            }
            renditions.push(video);
            for rendition in renditions {
                for key in [
                    "PlayAddr",
                    "playAddr",
                    "play_addr",
                    "PlayAddrStruct",
                    "playAddrStruct",
                ] {
                    let Some(address) = rendition.get(key) else {
                        continue;
                    };
                    let mut candidates = Vec::new();
                    address_urls(address, &mut candidates);
                    if !candidates.contains(&url) {
                        continue;
                    }
                    let field = |value: &Value, upper: &str, lower: &str| {
                        value
                            .get(upper)
                            .or_else(|| value.get(lower))
                            .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
                    };
                    format.width = field(address, "Width", "width")
                        .and_then(|n| n.try_into().ok())
                        .or(format.width);
                    format.height = field(address, "Height", "height")
                        .and_then(|n| n.try_into().ok())
                        .or(format.height);
                    format.filesize = field(address, "DataSize", "data_size").or(format.filesize);
                    format.bitrate = field(rendition, "Bitrate", "bitrate").or(format.bitrate);
                }
            }
            format
        })
        .collect();
    let mut covers = Vec::new();
    if let Some(cover) = video.get("cover").or_else(|| video.get("originCover")) {
        address_urls(cover, &mut covers);
    }
    let duration = video
        .get("duration")
        .and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()))
        .map(|n| {
            if item.get("aweme_id").is_some() {
                n / 1000.0
            } else {
                n
            }
        });
    Some(VideoMetadata {
        subtitles: Vec::new(),
        title: item
            .get("desc")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| direct::title_from_url(url)),
        thumbnail: covers.into_iter().next(),
        duration,
        uploader: item
            .pointer("/author/uniqueId")
            .or_else(|| item.pointer("/author/nickname"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        webpage_url: url.to_owned(),
        formats,
    })
}

pub(crate) fn from_html(
    html: &str,
    expected: Option<&str>,
    url: &str,
    headers: &Headers,
) -> Option<VideoMetadata> {
    for capture in SCRIPTS.captures_iter(html) {
        let attributes = &capture[1];
        if ![
            "__UNIVERSAL_DATA_FOR_REHYDRATION__",
            "__FRONTEND_DEFAULT_STATES__",
            "SIGI_STATE",
            "sigi-persisted-state",
            "sigi-persisted-data",
            "__NEXT_DATA__",
            "application/json",
        ]
        .iter()
        .any(|key| attributes.contains(key))
        {
            continue;
        }
        let json = &capture[2];
        if json.len() > 4 * 1024 * 1024 {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(json) {
            if let Some(mut meta) = metadata(&value, expected, url, headers) {
                if meta.title == direct::title_from_url(url) {
                    meta.title = site::page_title(html, url);
                }
                if meta.thumbnail.is_none() {
                    meta.thumbnail = site::page_thumbnail(html);
                }
                return Some(meta);
            }
        }
    }
    for found in STATE.find_iter(html) {
        let remaining = &html[found.end()..];
        if remaining.len() > 4 * 1024 * 1024 {
            continue;
        }
        if let Ok(value) = Value::deserialize(&mut serde_json::Deserializer::from_str(remaining)) {
            if let Some(meta) = metadata(&value, expected, url, headers) {
                return Some(meta);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const ID: &str = "7671362008486251790";
    const URL: &str = "https://www.tiktok.com/@creator/video/7671362008486251790?is_from_webapp=1&sender_device=pc";

    fn selected() -> Value {
        json!({"id": ID, "desc": "Selected clip", "author": {"uniqueId": "creator"}, "video": {
            "width": 1080, "height": 1920, "duration": 35,
            "cover": {"urlList": ["https://cdn.test/cover.jpg"]},
            "playAddr": {"urlList": ["https://cdn.test/play.mp4?token=a%2Fb+z"]},
            "downloadAddr": ["https://cdn.test/download.mp4"],
            "bitrateInfo": [{"PlayAddr": {"UrlList": ["https://cdn.test/hd.mp4"]}}]
        }})
    }

    #[test]
    fn modern_addresses_and_metadata_are_shared_without_credentials() {
        let headers = vec![
            ("Cookie".into(), "session=private".into()),
            ("Authorization".into(), "secret".into()),
            ("User-Agent".into(), "browser".into()),
        ];
        let payload = json!({"__DEFAULT_SCOPE__": {"webapp.video-detail": {"itemInfo": {"itemStruct": selected()}}}});
        let html = format!(
            "<script type='application/json' id='__UNIVERSAL_DATA_FOR_REHYDRATION__'>{payload}</script>"
        );
        let result = from_html(&html, Some(ID), URL, &headers).unwrap();
        assert_eq!(result.title, "Selected clip");
        assert_eq!(result.duration, Some(35.0));
        assert_eq!(result.uploader.as_deref(), Some("creator"));
        assert_eq!(
            result.thumbnail.as_deref(),
            Some("https://cdn.test/cover.jpg")
        );
        assert_eq!(result.formats.len(), 3);
        assert_eq!(
            result.formats[0].video_url.as_deref(),
            Some("https://cdn.test/play.mp4?token=a%2Fb+z")
        );
        assert!(result.formats.iter().all(|f| {
            f.height == Some(1920)
                && f.headers
                    .iter()
                    .all(|(k, _)| k != "Cookie" && k != "Authorization")
        }));
    }

    #[test]
    fn item_module_and_embed_payloads_select_the_requested_id() {
        let other = json!({"id": "7777777777777777777", "video": {"playAddr": "https://cdn.test/other.mp4"}});
        for payload in [
            json!({"ItemModule": { ID: selected(), "7777777777777777777": other.clone() }}),
            json!({"props": {"pageProps": {"itemList": [other, selected()]}}}),
        ] {
            let result = metadata(&payload, Some(ID), URL, &Headers::new()).unwrap();
            assert!(
                result
                    .formats
                    .iter()
                    .all(|f| !f.video_url.as_deref().unwrap().contains("other"))
            );
            assert!(
                metadata(&payload, Some("8888888888888888888"), URL, &Headers::new()).is_none()
            );
            assert!(metadata(&payload, None, URL, &Headers::new()).is_none());
        }
        let mut no_id = selected();
        no_id.as_object_mut().unwrap().remove("id");
        let payload = json!({"ItemModule": { ID: no_id }});
        assert!(metadata(&payload, Some(ID), URL, &Headers::new()).is_some());
        assert!(metadata(&payload, Some("7777777777777777777"), URL, &Headers::new()).is_none());
    }

    #[test]
    fn persisted_states_and_balanced_assignments_are_supported() {
        let payload = json!({"ItemModule": { ID: selected() }});
        for html in [
            format!("<script id='sigi-persisted-data'>{payload}</script>"),
            format!("<script>window.__INITIAL_STATE__ = {payload}; anotherCall();</script>"),
            format!("<script>window.__DEFAULT_SCOPE__ = {payload};</script>"),
        ] {
            assert_eq!(
                from_html(&html, Some(ID), URL, &Headers::new())
                    .unwrap()
                    .formats
                    .len(),
                3
            );
        }
    }

    #[test]
    fn mobile_lists_and_duration_units_are_supported() {
        let payload = json!({"aweme_list": [{"aweme_id": ID, "video": {
            "duration": 35000, "play_addr": {"url_list": ["https://cdn.test/mobile.mp4"]},
            "bit_rate": [{"play_addr": {"url_list": ["https://cdn.test/mobile-hd.mp4"]}}]
        }}]});
        let result = metadata(&payload, Some(ID), URL, &Headers::new()).unwrap();
        assert_eq!(result.duration, Some(35.0));
        assert_eq!(result.formats.len(), 2);
    }

    #[test]
    fn poster_music_unsafe_urls_and_ambiguous_items_are_rejected() {
        let video = json!({"playAddr": ["blob:https://www.tiktok.com/123", "javascript:alert(1)",
            "https://cdn.test/cover.jpg", "https://cdn.test/song.mp3", "https://cdn.test/clip.mp4"],
            "music": {"playUrl": "https://cdn.test/song.mp3"}});
        assert_eq!(video_urls(&video), ["https://cdn.test/clip.mp4"]);
        let payload = json!({"items": [{"video": video.clone()}, {"video": video}]});
        assert!(metadata(&payload, Some(ID), URL, &Headers::new()).is_none());
    }
}

#[cfg(test)]
#[test]
fn real_web_rendition_structures_keep_their_own_quality_and_size() {
    let value = serde_json::json!({"id": "7671362008486251790", "video": {
        "width": 720, "height": 1280,
        "PlayAddrStruct": {"Width": 720, "Height": 1280, "DataSize": 1200000, "UrlList": ["https://cdn.test/720.mp4"]},
        "bitrateInfo": [{"Bitrate": 1800000, "PlayAddr": {"Width": 1080, "Height": 1920, "DataSize": 2500000,
            "UrlList": ["https://cdn.test/1080.mp4"]}}]
    }});
    let meta = metadata(
        &value,
        Some("7671362008486251790"),
        "https://www.tiktok.com/@_/video/7671362008486251790",
        &Headers::new(),
    )
    .unwrap();
    let primary = meta
        .formats
        .iter()
        .find(|f| f.video_url.as_deref().unwrap().ends_with("720.mp4"))
        .unwrap();
    let hd = meta
        .formats
        .iter()
        .find(|f| f.video_url.as_deref().unwrap().ends_with("1080.mp4"))
        .unwrap();
    assert_eq!(
        (primary.width, primary.height, primary.filesize),
        (Some(720), Some(1280), Some(1200000))
    );
    assert_eq!(
        (hd.width, hd.height, hd.filesize, hd.bitrate),
        (Some(1080), Some(1920), Some(2500000), Some(1800000))
    );
}
