//! Bandcamp: tracks and albums.
//!
//! A Bandcamp page carries its whole player state in a `data-tralbum`
//! attribute: every track's title, number, length, and — for anything the
//! artist lets stream — a 128 kbps MP3 URL. That is the same stream the page's
//! own player plays, and it is what is offered here: Bandcamp's higher-quality
//! files are what you buy, and nothing in this module goes near them.
//!
//! Stream URLs are signed and expire, so a track resolves to
//! [`AudioSource::Bandcamp`] and the page is re-read at download time.

use reqwest::Client;
use serde_json::Value;

use super::{AudioSource, MusicSource, MusicTrack, Service, SourceKind, get_html, json_str};
use crate::error::{Error, Result};

/// The `data-tralbum` JSON of a page.
fn tralbum(html: &str) -> Option<Value> {
    let at = html.find("data-tralbum=\"")? + "data-tralbum=\"".len();
    let end = html[at..].find('"')? + at;
    let raw = &html[at..end];
    let decoded = raw
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    serde_json::from_str(&decoded).ok()
}

/// `16 Sep 2010 00:00:00 GMT` → `2010-09-16`.
fn iso_date(raw: &str) -> Option<String> {
    chrono::NaiveDateTime::parse_from_str(raw.trim_end_matches(" GMT"), "%d %b %Y %H:%M:%S")
        .ok()
        .map(|d| d.format("%Y-%m-%d").to_string())
}

fn art_url(art_id: Option<u64>) -> Option<String> {
    art_id.map(|id| format!("https://f4.bcbits.com/img/a{id}_10.jpg"))
}

/// `class="fromAlbum">Album Name<` on a track page.
fn album_from_page(html: &str) -> Option<String> {
    let at = html.find("class=\"fromAlbum\">")? + "class=\"fromAlbum\">".len();
    let end = html[at..].find('<')? + at;
    let name = html[at..end].trim();
    (!name.is_empty()).then(|| name.replace("&amp;", "&").replace("&#39;", "'").replace("&quot;", "\""))
}

fn origin_of(url: &str) -> String {
    let without = url.trim_start_matches("https://").trim_start_matches("http://");
    let host = without.split('/').next().unwrap_or(without);
    format!("https://{host}")
}

pub async fn analyze(client: &Client, url: &str) -> Result<MusicSource> {
    let url = url.trim().split(['?', '#']).next().unwrap_or(url).to_string();
    let html = get_html(client, &url).await?;
    let data = tralbum(&html).ok_or_else(|| {
        Error::InvalidInput("this Bandcamp page has no player data — paste an album or track page, not the artist's front page".into())
    })?;
    let album_from_html = album_from_page(&html);
    build(&data, &url, album_from_html)
}

fn build(data: &Value, url: &str, album_from_html: Option<String>) -> Result<MusicSource> {
    let item_type = json_str(data, "/item_type").unwrap_or_default();
    let artist = json_str(data, "/artist");
    let current_title = json_str(data, "/current/title").unwrap_or_else(|| "Untitled".into());
    let release = json_str(data, "/album_release_date").or_else(|| json_str(data, "/current/release_date"));
    let date = release.as_deref().and_then(iso_date);
    let year = date.as_deref().and_then(super::year_of);
    let cover = art_url(data.get("art_id").and_then(Value::as_u64).or_else(|| data.pointer("/current/art_id").and_then(Value::as_u64)));
    let origin = origin_of(url);
    let is_album = item_type == "album";
    let album_name = if is_album { Some(current_title.clone()) } else { album_from_html };

    let items = data.get("trackinfo").and_then(Value::as_array).cloned().unwrap_or_default();
    let total = items.len() as u32;
    let tracks: Vec<MusicTrack> = items
        .iter()
        .filter_map(|t| {
            let id = t.get("track_id").or_else(|| t.get("id")).and_then(Value::as_u64)?;
            let title = json_str(t, "/title")?;
            let streamable = json_str(t, "/file/mp3-128").is_some();
            let page = json_str(t, "/title_link").map(|l| if l.starts_with('/') { format!("{origin}{l}") } else { l }).unwrap_or_else(|| url.to_string());
            Some(MusicTrack {
                id: format!("bc:{id}"),
                title,
                artists: json_str(t, "/artist").or_else(|| artist.clone()).into_iter().collect(),
                album: album_name.clone(),
                album_artist: is_album.then(|| artist.clone()).flatten(),
                track_number: t.get("track_num").and_then(Value::as_u64).map(|n| n as u32),
                track_total: is_album.then_some(total),
                disc_number: None,
                year,
                date: date.clone(),
                genre: None,
                duration: t.get("duration").and_then(Value::as_f64).filter(|d| *d > 0.0),
                cover_url: cover.clone(),
                isrc: None,
                source_url: page.clone(),
                service: Service::Bandcamp,
                source: AudioSource::Bandcamp { page_url: page, track_id: id },
                explicit: false,
                unavailable: (!streamable).then(|| "the artist has not made this track streamable".to_string()),
            })
        })
        .collect();

    if tracks.is_empty() {
        return Err(Error::InvalidInput("this Bandcamp page lists no tracks".into()));
    }
    Ok(MusicSource {
        kind: if is_album { SourceKind::Album } else { SourceKind::Track },
        service: Service::Bandcamp,
        title: current_title,
        owner: artist,
        cover_url: cover,
        source_url: json_str(data, "/url").unwrap_or_else(|| url.to_string()),
        tracks,
        total: is_album.then_some(total),
        note: Some("Bandcamp streams at 128 kbps MP3; the higher-quality files are the ones you buy".into()),
    })
}

/// A fresh stream URL for a track on a page.
pub async fn stream_url(client: &Client, page_url: &str, track_id: u64) -> Result<String> {
    let html = get_html(client, page_url).await?;
    let data = tralbum(&html).ok_or_else(|| Error::InvalidInput("the Bandcamp page no longer carries its player data".into()))?;
    let items = data.get("trackinfo").and_then(Value::as_array).cloned().unwrap_or_default();
    items
        .iter()
        .find(|t| t.get("track_id").or_else(|| t.get("id")).and_then(Value::as_u64) == Some(track_id))
        .and_then(|t| json_str(t, "/file/mp3-128"))
        .ok_or_else(|| Error::InvalidInput("Bandcamp offers no stream for this track — the artist may have made it purchase-only".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_player_data_is_decoded_from_the_attribute() {
        let html = r#"<div data-tralbum="{&quot;current&quot;:{&quot;title&quot;:&quot;Beacons &amp; more&quot;},&quot;trackinfo&quot;:[]}" data-x="1">"#;
        let data = tralbum(html).unwrap();
        assert_eq!(data["current"]["title"], "Beacons & more");
    }

    #[test]
    fn release_dates_and_art_are_derived() {
        assert_eq!(iso_date("16 Sep 2010 00:00:00 GMT").as_deref(), Some("2010-09-16"));
        assert_eq!(art_url(Some(3305708557)).as_deref(), Some("https://f4.bcbits.com/img/a3305708557_10.jpg"));
        assert_eq!(album_from_page(r#"<span class="fromAlbum">Beacons</span>"#).as_deref(), Some("Beacons"));
    }

    #[test]
    fn an_album_page_yields_numbered_tracks() {
        let data = json!({
            "item_type": "album", "artist": "Cloudkicker", "art_id": 3305708557u64, "album_release_date": "16 Sep 2010 00:00:00 GMT",
            "url": "https://cloudkicker.bandcamp.com/album/beacons", "current": {"title": "Beacons"},
            "trackinfo": [
                {"id": 1, "track_id": 1, "title": "We are going to invert...", "track_num": 1, "duration": 41.4, "title_link": "/track/invert", "file": {"mp3-128": "https://t4.bcbits.com/stream/x"}},
                {"id": 2, "track_id": 2, "title": "Purchase only", "track_num": 2, "duration": 100.0, "title_link": "/track/po", "file": null}
            ]
        });
        let source = build(&data, "https://cloudkicker.bandcamp.com/album/beacons", None).unwrap();
        assert_eq!(source.kind, SourceKind::Album);
        assert_eq!(source.owner.as_deref(), Some("Cloudkicker"));
        assert_eq!(source.total, Some(2));
        let t = &source.tracks[0];
        assert_eq!(t.album.as_deref(), Some("Beacons"));
        assert_eq!(t.track_number, Some(1));
        assert_eq!(t.year, Some(2010));
        assert_eq!(t.source_url, "https://cloudkicker.bandcamp.com/track/invert");
        assert_eq!(t.source, AudioSource::Bandcamp { page_url: "https://cloudkicker.bandcamp.com/track/invert".into(), track_id: 1 });
        assert!(source.tracks[1].unavailable.is_some());
    }

    #[test]
    fn a_track_page_takes_its_album_from_the_page() {
        let data = json!({"item_type": "track", "artist": "Cloudkicker", "art_id": 5, "current": {"title": "Invert", "track_number": 1},
            "trackinfo": [{"track_id": 1, "title": "Invert", "track_num": 1, "duration": 41.4, "file": {"mp3-128": "https://s"}}]});
        let source = build(&data, "https://cloudkicker.bandcamp.com/track/invert", Some("Beacons".into())).unwrap();
        assert_eq!(source.kind, SourceKind::Track);
        assert_eq!(source.tracks[0].album.as_deref(), Some("Beacons"));
        assert_eq!(source.tracks[0].source_url, "https://cloudkicker.bandcamp.com/track/invert");
    }
}
