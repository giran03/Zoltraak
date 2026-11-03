//! Lyrics from lrclib.net.
//!
//! A free, unauthenticated, community-maintained lyrics database with a
//! deliberately small API: one GET by artist, title, album and duration. It
//! returns both plain lyrics and, when someone has timed them, LRC-format
//! synced lyrics — which is what a music player scrolls along to.
//!
//! Optional in every sense: it is off by default, a miss is not an error, and
//! nothing about the user's library is sent beyond the track's own metadata.

use reqwest::Client;
use serde::Deserialize;

use super::MusicTrack;
use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Lyrics {
    pub plain: Option<String>,
    /// `[mm:ss.xx] line` format, when timed lyrics exist.
    pub synced: Option<String>,
}

impl Lyrics {
    pub fn is_empty(&self) -> bool {
        self.plain.is_none() && self.synced.is_none()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Hit {
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
    #[serde(default)]
    instrumental: bool,
}

const ENDPOINT: &str = "https://lrclib.net/api/get";

/// Look the track up. `Ok(None)` when lrclib has nothing for it.
pub async fn fetch(client: &Client, track: &MusicTrack) -> Result<Option<Lyrics>> {
    let Some(artist) = track.primary_artist() else { return Ok(None) };
    let mut query: Vec<(&str, String)> = vec![("artist_name", artist.to_string()), ("track_name", track.title.clone())];
    if let Some(album) = &track.album {
        query.push(("album_name", album.clone()));
    }
    if let Some(d) = track.duration {
        query.push(("duration", format!("{}", d.round() as u64)));
    }

    let response = client
        .get(ENDPOINT)
        .query(&query)
        // lrclib asks clients to identify themselves.
        .header("User-Agent", "Zoltraak (https://github.com/giran03/Zoltraak)")
        .send()
        .await?;
    if response.status().as_u16() == 404 {
        return Ok(None);
    }
    if !response.status().is_success() {
        tracing::debug!(status = %response.status(), "lrclib lookup failed");
        return Ok(None);
    }
    let hit: Hit = match response.json().await {
        Ok(hit) => hit,
        Err(e) => {
            tracing::debug!(error = %e, "lrclib answer was not the expected JSON");
            return Ok(None);
        }
    };
    if hit.instrumental {
        return Ok(None);
    }
    let lyrics = Lyrics {
        plain: hit.plain_lyrics.filter(|s| !s.trim().is_empty()),
        synced: hit.synced_lyrics.filter(|s| !s.trim().is_empty()),
    };
    Ok((!lyrics.is_empty()).then_some(lyrics))
}
