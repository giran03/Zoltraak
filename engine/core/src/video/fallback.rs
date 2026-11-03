//! Explicit local yt-dlp extraction only. Downloads stay in the engine.
use super::{Subtitle, VideoFormat, VideoMetadata};
use crate::error::{Error, Result};
use serde_json::Value;

pub async fn analyze(url: &str) -> Result<VideoMetadata> {
    let parsed =
        reqwest::Url::parse(url).map_err(|_| Error::InvalidInput("Invalid video URL".into()))?;
    if !["http", "https"].contains(&parsed.scheme()) {
        return Err(Error::InvalidInput(
            "Fallback requires an HTTP video page".into(),
        ));
    }
    let mut command = tokio::process::Command::new("yt-dlp");
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    command
        .kill_on_drop(true)
        .args([
            "--ignore-config",
            "--no-plugin-dirs",
            "--no-remote-components",
            "--no-playlist",
            "--skip-download",
            "--dump-single-json",
            "--no-warnings",
            "--socket-timeout",
            "10",
            "--retries",
            "1",
            "--extractor-retries",
            "1",
            "--",
        ])
        .arg(url)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().map_err(|error| Error::InvalidInput(if error.kind() == std::io::ErrorKind::NotFound {
        "Optional yt-dlp was not found on PATH. Install it separately, or use the built-in extractor.".into()
    } else { "Could not launch the optional yt-dlp extractor".into() }))?;
    let output = tokio::time::timeout(std::time::Duration::from_secs(45), async {
        use tokio::io::AsyncReadExt;
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let read_stdout = async {
            let mut bytes = Vec::new();
            (&mut stdout).take(20 * 1024 * 1024 + 1).read_to_end(&mut bytes).await?;
            if bytes.len() > 20 * 1024 * 1024 {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "metadata limit"));
            }
            Ok::<_, std::io::Error>(bytes)
        };
        let read_stderr = async {
            // Drain concurrently to avoid pipe deadlocks; retain only a
            // bounded diagnostic sample. Raw stderr is never shown or logged.
            let mut sample = Vec::new();
            let mut chunk = [0; 4096];
            loop {
                let count = stderr.read(&mut chunk).await?;
                if count == 0 { break; }
                let keep = count.min(16384usize.saturating_sub(sample.len()));
                sample.extend_from_slice(&chunk[..keep]);
            }
            Ok::<_, std::io::Error>(sample)
        };
        let (bytes, diagnostic) = tokio::try_join!(read_stdout, read_stderr).map_err(|error| Error::InvalidInput(
            if error.kind() == std::io::ErrorKind::InvalidData { "yt-dlp metadata exceeds the extraction limit" }
            else { "Could not read yt-dlp metadata." }.into()))?;
        if bytes.len() > 20 * 1024 * 1024 { return Err(Error::InvalidInput("yt-dlp metadata exceeds the extraction limit".into())); }
        let status = child.wait().await.map_err(|e| Error::InvalidInput(e.to_string()))?;
        if !status.success() { return Err(Error::InvalidInput(failure_message(&diagnostic))); }
        parse(&bytes, url)
    }).await.map_err(|_| Error::InvalidInput("Optional yt-dlp extraction timed out".into()))?;
    output
}

fn failure_message(stderr: &[u8]) -> String {
    let diagnostic = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    let reason = if diagnostic.contains("429") || diagnostic.contains("rate limit") || diagnostic.contains("too many requests") {
        "Instagram or the source site rate-limited yt-dlp. Wait before retrying."
    } else if diagnostic.contains("login") || diagnostic.contains("sign in") || diagnostic.contains("cookies") || diagnostic.contains("private") {
        "yt-dlp requires a sign-in for this source."
    } else if diagnostic.contains("unsupported url") {
        "The installed yt-dlp does not support this URL."
    } else if diagnostic.contains("javascript runtime") || diagnostic.contains("js runtime") {
        "The installed yt-dlp could not find its required JavaScript runtime."
    } else { "yt-dlp could not resolve this source; it may be unavailable or its response is unsupported." };
    format!("{reason} This optional tool does not receive shared browser credentials; use the built-in extractor for shared sign-ins.")
}

fn http(value: &Value) -> Option<String> {
    let text = value.as_str()?;
    let url = reqwest::Url::parse(text).ok()?;
    if ["http", "https"].contains(&url.scheme()) {
        Some(text.to_string())
    } else {
        None
    }
}
pub(crate) fn parse(bytes: &[u8], page: &str) -> Result<VideoMetadata> {
    let info: Value = serde_json::from_slice(bytes)
        .map_err(|_| Error::InvalidInput("yt-dlp returned invalid metadata".into()))?;
    if info.get("entries").is_some() || info["is_live"] == true || info["has_drm"] == true {
        return Err(Error::InvalidInput(
            "Fallback accepts a single clear, non-live video only".into(),
        ));
    }
    if let Some(expected) = super::validation::content_identity(page) {
        let actual = info["webpage_url"]
            .as_str()
            .and_then(super::validation::content_identity);
        if actual.as_ref() != Some(&expected) {
            return Err(Error::InvalidInput(
                "Fallback did not establish the requested video identity".into(),
            ));
        }
    }
    let empty = Vec::new();
    let sources = info["formats"].as_array().unwrap_or(&empty);
    let usable = |f: &&Value| {
        f["has_drm"] != true
            && http(&f["url"]).is_some()
            && (f["protocol"] != "m3u8_native"
                || http(&f["url"])
                    .and_then(|s| reqwest::Url::parse(&s).ok())
                    .is_some_and(|u| u.path().ends_with(".m3u8")))
            && ["http", "https", "m3u8_native"].contains(&f["protocol"].as_str().unwrap_or("https"))
    };
    let mut formats = Vec::new();
    for f in sources
        .iter()
        .filter(usable)
        .filter(|f| f["vcodec"] != "none")
    {
        let Some(source) = http(&f["url"]) else {
            continue;
        };
        if f["protocol"] == "m3u8_native"
            && !reqwest::Url::parse(&source)
                .ok()
                .is_some_and(|u| u.path().ends_with(".m3u8"))
        {
            continue;
        }
        let has_audio = f["acodec"].as_str().map(|c| c != "none");
        let pair = if has_audio == Some(false) {
            sources
                .iter()
                .filter(usable)
                .filter(|a| {
                    a["vcodec"] == "none"
                        && a["acodec"].as_str().is_some_and(|c| c != "none")
                        && (f["ext"] != "webm"
                            || a["ext"] == "webm"
                            || a["acodec"]
                                .as_str()
                                .is_some_and(|c| c.starts_with("opus") || c.starts_with("vorbis")))
                })
                .max_by(|a, b| {
                    let rank = |a: &Value| {
                        if f["ext"] != "webm"
                            && a["acodec"]
                                .as_str()
                                .is_some_and(|c| c.starts_with("mp4a") || c.starts_with("aac"))
                        {
                            1
                        } else {
                            0
                        }
                    };
                    rank(a).cmp(&rank(b)).then_with(|| {
                        a["abr"]
                            .as_f64()
                            .unwrap_or(0.0)
                            .total_cmp(&b["abr"].as_f64().unwrap_or(0.0))
                    })
                })
        } else {
            None
        };
        let safe_headers = f["http_headers"]
            .as_object()
            .map(|h| {
                h.iter()
                    .filter(|(k, _)| {
                        [
                            "user-agent",
                            "referer",
                            "origin",
                            "accept",
                            "accept-language",
                        ]
                        .contains(&k.to_ascii_lowercase().as_str())
                    })
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.into())))
                    .collect()
            })
            .unwrap_or_default();
        formats.push(VideoFormat {
            id: format!("yt-dlp-{}", f["format_id"].as_str().unwrap_or("video")),
            container: if f["ext"] == "webm" {
                "webm".into()
            } else {
                "mp4".into()
            },
            video_codec: f["vcodec"].as_str().map(str::to_owned),
            audio_codec: pair.unwrap_or(f)["acodec"]
                .as_str()
                .filter(|c| *c != "none")
                .map(str::to_owned),
            width: f["width"].as_u64().and_then(|v| v.try_into().ok()),
            height: f["height"].as_u64().and_then(|v| v.try_into().ok()),
            fps: f["fps"].as_f64().map(|v| v as f32),
            bitrate: f["tbr"].as_f64().map(|v| (v * 1000.0) as u64),
            filesize: if pair.is_none() {
                f["filesize"].as_u64()
            } else {
                None
            },
            video_url: Some(source),
            audio_url: pair.and_then(|a| http(&a["url"])),
            requires_merge: pair.is_some(),
            has_audio,
            headers: safe_headers,
            ..Default::default()
        });
    }
    if formats.is_empty() {
        return Err(Error::InvalidInput(
            "yt-dlp found no supported clear HTTP formats".into(),
        ));
    }
    let mut subtitles = Vec::new();
    for (key, automatic) in [("subtitles", false), ("automatic_captions", true)] {
        if let Some(languages) = info[key].as_object() {
            for (language, list) in languages {
                let Some(entries) = list.as_array() else {
                    continue;
                };
                let Some(entry) = entries.iter().find(|s| {
                    ["vtt", "srt"].contains(&s["ext"].as_str().unwrap_or(""))
                        && http(&s["url"]).is_some()
                }) else {
                    continue;
                };
                let subtitle = Subtitle {
                    language: language.clone(),
                    label: entry["name"].as_str().unwrap_or(language).into(),
                    automatic,
                    url: http(&entry["url"]).unwrap(),
                    extension: entry["ext"].as_str().unwrap().into(),
                };
                if super::subtitles::validate(&subtitle).is_ok() {
                    subtitles.push(subtitle);
                }
            }
        }
    }
    Ok(VideoMetadata {
        title: info["title"].as_str().unwrap_or("Video").into(),
        thumbnail: http(&info["thumbnail"]),
        duration: info["duration"]
            .as_f64()
            .filter(|d| d.is_finite() && *d > 0.0),
        uploader: info["uploader"].as_str().map(str::to_owned),
        webpage_url: page.into(),
        formats,
        subtitles,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_diagnostics_classify_errors_without_echoing_sensitive_output() {
        let message = failure_message(b"ERROR: HTTP 429 https://instagram.com/reel/ABC/?secret=token Cookie: session=private");
        assert!(message.contains("rate-limited"));
        assert!(!message.contains("secret") && !message.contains("session=private"));
        assert!(failure_message(b"ERROR: Login required; pass cookies").contains("requires a sign-in"));
        assert!(failure_message(b"ERROR: Unsupported URL").contains("does not support"));
    }
    #[test]
    fn fallback_rejects_playlists_live_drm_and_identity_changes() {
        for value in [
            serde_json::json!({"entries": []}),
            serde_json::json!({"is_live": true}),
            serde_json::json!({"has_drm": true}),
            serde_json::json!({"webpage_url": "https://youtube.com/watch?v=other"}),
        ] {
            assert!(
                parse(
                    &serde_json::to_vec(&value).unwrap(),
                    "https://youtube.com/watch?v=wanted"
                )
                .is_err()
            );
        }
    }
    #[test]
    fn fallback_pairs_tracks_keeps_signed_urls_and_exposes_direct_captions() {
        let info = serde_json::json!({"title":"Fixture", "webpage_url":"https://youtube.com/watch?v=wanted", "formats":[
            {"format_id":"v","url":"https://cdn.test/v.mp4?sig=x","protocol":"https","vcodec":"avc1","acodec":"none","height":720,"ext":"mp4","http_headers":{"User-Agent":"fixture","Cookie":"secret"}},
            {"format_id":"a","url":"https://cdn.test/a.m4a?sig=y","protocol":"https","vcodec":"none","acodec":"mp4a","abr":128}],
            "subtitles":{"en":[{"url":"https://cdn.test/en.vtt","ext":"vtt"}]}});
        let meta = parse(
            &serde_json::to_vec(&info).unwrap(),
            "https://youtube.com/watch?v=wanted",
        )
        .unwrap();
        assert_eq!(
            meta.formats[0].video_url.as_deref(),
            Some("https://cdn.test/v.mp4?sig=x")
        );
        assert!(meta.formats[0].requires_merge);
        assert_eq!(
            meta.formats[0].headers,
            vec![("User-Agent".into(), "fixture".into())]
        );
        assert_eq!(meta.subtitles[0].language, "en");
    }
}
