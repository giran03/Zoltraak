//! Built-in adapters share one routing and extraction contract. Validation
//! remains in the caller so every adapter receives the same safety checks.
use super::{VideoMetadata, dash, direct, hls, site};
use crate::{
    error::{Error, Result},
    headers::Headers,
};
use reqwest::Client;

#[derive(Debug, Clone, Copy)]
pub enum Extractor {
    Hls,
    Dash,
    Progressive,
    Site(site::Platform),
    Page,
}

impl Extractor {
    pub fn for_url(url: &str) -> Self {
        let path = url
            .split(['?', '#'])
            .next()
            .unwrap_or(url)
            .to_ascii_lowercase();
        if path.ends_with(".m3u8") {
            Self::Hls
        } else if path.ends_with(".mpd") {
            Self::Dash
        } else if direct::is_direct_media(url) {
            Self::Progressive
        } else if let Some(platform) = site::matches_platform(url) {
            Self::Site(platform)
        } else {
            Self::Page
        }
    }

    pub async fn extract(
        self,
        client: &Client,
        url: &str,
        headers: &Headers,
    ) -> Result<VideoMetadata> {
        match self {
            Self::Hls => hls::analyze(client, url, headers).await,
            Self::Dash => dash::analyze(client, url, headers).await,
            Self::Progressive => Ok(direct::single_format(url)),
            Self::Page => direct::analyze_page(client, url, headers).await,
            Self::Site(platform) => {
                let error = match site::analyze(client, url, headers, platform).await {
                    Ok(meta) if !meta.formats.is_empty() => return Ok(meta),
                    Ok(_) => Error::InvalidInput(format!("no downloadable video found on {url}")),
                    Err(error) => error,
                };
                if matches!(
                    platform,
                    site::Platform::YouTube | site::Platform::Instagram | site::Platform::TikTok
                ) {
                    return Err(error);
                }
                match direct::analyze_page(client, url, headers).await {
                    Ok(meta) if !meta.formats.is_empty() => Ok(meta),
                    _ => Err(error),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn routes_signed_manifests_and_platform_pages_without_changing_urls() {
        assert!(matches!(
            Extractor::for_url("https://cdn.example/a.MPD?token=x"),
            Extractor::Dash
        ));
        assert!(matches!(
            Extractor::for_url("https://cdn.example/a.m3u8?x=1"),
            Extractor::Hls
        ));
        assert!(matches!(
            Extractor::for_url("https://cdn.example/a.mp4?x=1"),
            Extractor::Progressive
        ));
        assert!(matches!(
            Extractor::for_url("https://cdn.example/download?filename=clip.MP4&token=signed"),
            Extractor::Progressive
        ));
        assert!(matches!(
            Extractor::for_url("https://cdn.example/download?format=webm&token=signed"),
            Extractor::Progressive
        ));
        assert!(matches!(
            Extractor::for_url("https://youtube.com/watch?v=id"),
            Extractor::Site(site::Platform::YouTube)
        ));
        assert!(matches!(
            Extractor::for_url("https://youtube.com.evil.example/watch?v=id"),
            Extractor::Page
        ));
    }
}
