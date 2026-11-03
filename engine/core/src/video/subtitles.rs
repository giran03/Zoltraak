use super::Subtitle;
use crate::{
    error::{Error, Result},
    headers::{self, Headers},
};
use futures_util::StreamExt;
use std::path::{Path, PathBuf};

pub fn validate(subtitle: &Subtitle) -> Result<()> {
    let url = reqwest::Url::parse(&subtitle.url)
        .map_err(|_| Error::InvalidInput("Invalid subtitle URL".into()))?;
    if !["http", "https"].contains(&url.scheme())
        || !["vtt", "srt"].contains(&subtitle.extension.as_str())
        || subtitle.language.is_empty()
        || subtitle.language.len() > 40
        || !subtitle
            .language
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(Error::InvalidInput(
            "Choose a direct VTT or SRT subtitle with a valid language code".into(),
        ));
    }
    Ok(())
}

pub async fn save(
    client: &reqwest::Client,
    subtitle: &Subtitle,
    page: &str,
    supplied: &Headers,
    video: &Path,
) -> Result<PathBuf> {
    validate(subtitle)?;
    let same_origin = reqwest::Url::parse(page).ok().map(|u| u.origin())
        == reqwest::Url::parse(&subtitle.url).ok().map(|u| u.origin());
    let safe: Headers = supplied
        .iter()
        .filter(|(key, _)| {
            same_origin
                && ["cookie", "authorization", "origin"]
                    .contains(&key.to_ascii_lowercase().as_str())
                || ["user-agent", "referer", "accept", "accept-language"]
                    .contains(&key.to_ascii_lowercase().as_str())
        })
        .cloned()
        .collect();
    let data = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        let response = headers::apply(client.get(&subtitle.url), &safe)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(Error::UnexpectedStatus {
                status: response.status().as_u16(),
                url: subtitle.url.clone(),
            });
        }
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
                return Err(Error::InvalidInput("Subtitle exceeds 2 MiB".into()));
            }
            bytes.extend_from_slice(&chunk);
        }
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| Error::InvalidInput("Subtitle is not UTF-8 text".into()))?;
        if !text.contains("-->")
            || (subtitle.extension == "vtt"
                && !text
                    .trim_start_matches('\u{feff}')
                    .trim_start()
                    .starts_with("WEBVTT"))
        {
            return Err(Error::InvalidInput(
                "Subtitle endpoint returned invalid caption text".into(),
            ));
        }
        Ok(bytes)
    })
    .await
    .map_err(|_| {
        Error::InvalidInput("Subtitle download timed out; the video was retained".into())
    })??;
    let path = video.with_file_name(format!(
        "{}.{}.{}",
        video.file_name().unwrap_or_default().to_string_lossy(),
        subtitle.language,
        subtitle.extension
    ));
    // A bounded local commit completes even if the caller pauses during it;
    // cancellation must not leave a half-written final caption file.
    tokio::task::spawn_blocking(move || {
        if let Ok(metadata) = std::fs::metadata(&path) {
            if metadata.len() == data.len() as u64
                && std::fs::read(&path).is_ok_and(|existing| existing == data)
            {
                return Ok(path);
            }
            return Err(Error::InvalidInput(
                "A different subtitle file already exists beside this video".into(),
            ));
        }
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|source| Error::Io {
                path: path.clone(),
                source,
            })?;
        if let Err(source) = file.write_all(&data).and_then(|_| file.flush()) {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(Error::Io { path, source });
        }
        Ok(path)
    })
    .await
    .map_err(|_| {
        Error::InvalidInput("Subtitle file commit failed; the video was retained".into())
    })?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn html_tracks_resolve_relative_urls_deduplicate_and_reject_unsafe_languages() {
        let html = r#"<video><track kind="subtitles" src="subs/en.vtt?x=1&amp;y=2" srclang="en" label="English"><track kind="captions" src="subs/en.vtt?x=1&amp;y=2" srclang="en"><track kind="subtitles" src="file:///secret" srclang="en"><track kind="subtitles" src="subs/fr.vtt" srclang="../fr"></video>"#;
        let tracks = from_html(html, "https://fixture.test/watch");
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].url, "https://fixture.test/subs/en.vtt?x=1&y=2");
        assert_eq!(tracks[0].label, "English");
    }
}

/// Native HTML tracks only; caption playlists are not mislabeled as text.
pub fn from_html(html: &str, page: &str) -> Vec<Subtitle> {
    let tag = regex::Regex::new(r"(?is)<track\b[^>]*>").unwrap();
    let attr = regex::Regex::new(r#"(?i)([\w-]+)\s*=\s*["']([^"']*)["']"#).unwrap();
    let mut out = Vec::new();
    for tag in tag.find_iter(html) {
        let attrs: std::collections::HashMap<_, _> = attr
            .captures_iter(tag.as_str())
            .map(|c| {
                (
                    c[1].to_ascii_lowercase(),
                    super::direct::unescape_url(&c[2]),
                )
            })
            .collect();
        if !attrs
            .get("kind")
            .is_some_and(|kind| ["subtitles", "captions"].contains(&kind.as_str()))
        {
            continue;
        }
        let Some(src) = attrs.get("src") else {
            continue;
        };
        let Some(url) = reqwest::Url::parse(page)
            .ok()
            .and_then(|base| base.join(src).ok())
        else {
            continue;
        };
        let extension = if url.path().ends_with(".srt") {
            "srt"
        } else {
            "vtt"
        };
        let language = attrs
            .get("srclang")
            .cloned()
            .unwrap_or_else(|| "und".into());
        let subtitle = Subtitle {
            label: attrs
                .get("label")
                .cloned()
                .unwrap_or_else(|| language.clone()),
            language,
            url: url.to_string(),
            extension: extension.into(),
            automatic: false,
        };
        if validate(&subtitle).is_ok() && !out.iter().any(|s: &Subtitle| s.url == subtitle.url) {
            out.push(subtitle);
        }
    }
    out
}
