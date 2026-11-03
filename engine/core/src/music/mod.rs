//! Music capture: a catalogue link in, tagged audio files out.
//!
//! A music link is rarely a link to a file. A Spotify or Apple Music URL names
//! a *track* — title, artists, album, duration, cover — and the audio behind it
//! is DRM-protected and never served to anything but their own players. So a
//! music download is two problems, not one:
//!
//! 1. **Resolve the catalogue** ([`analyze`]): turn the URL into a
//!    [`MusicSource`] — one track, or an album/playlist of them — with every
//!    piece of metadata the service will tell an anonymous client. Each
//!    service has its own small extractor ([`spotify`], [`apple`], [`deezer`],
//!    [`ytmusic`], [`soundcloud`], [`bandcamp`], [`direct`]).
//! 2. **Obtain the audio** ([`manager`]): each track carries an
//!    [`AudioSource`] saying where its sound comes from. For the DRM services
//!    that is [`AudioSource::Match`] — the track is looked up on YouTube Music
//!    by title, artist and duration and the best-scoring result's audio-only
//!    stream is taken through the existing YouTube extractor. YouTube,
//!    SoundCloud, Bandcamp and plain files hand over a stream directly.
//!
//! The result is then run through a local `ffmpeg` ([`convert`]) to land in
//! the format the user chose, with the catalogue's metadata and cover art
//! written into the file — which is the part that makes a folder of downloads
//! into a library rather than a folder of `videoplayback.m4a`.
//!
//! What this deliberately does *not* do: touch a service's protected stream.
//! Nothing here decrypts anything. The catalogue lookups use the same public
//! pages and endpoints a browser loads without signing in.

pub mod apple;
pub mod artwork;
pub mod bandcamp;
pub mod convert;
pub mod deezer;
pub mod direct;
pub mod lyrics;
pub mod manager;
pub mod name;
pub mod soundcloud;
pub mod spotify;
pub mod ytmusic;

use std::path::PathBuf;

use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::headers::Headers;

/// Where a link (or a track) came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Service {
    Spotify,
    AppleMusic,
    Deezer,
    #[serde(rename = "youtube")]
    YouTube,
    #[serde(rename = "youtube_music")]
    YouTubeMusic,
    #[serde(rename = "soundcloud")]
    SoundCloud,
    Bandcamp,
    /// A direct link to an audio file or stream, or a page that embeds one.
    Direct,
    /// Free-text search rather than a link.
    Search,
}

impl Service {
    pub fn label(self) -> &'static str {
        match self {
            Self::Spotify => "Spotify",
            Self::AppleMusic => "Apple Music",
            Self::Deezer => "Deezer",
            Self::YouTube => "YouTube",
            Self::YouTubeMusic => "YouTube Music",
            Self::SoundCloud => "SoundCloud",
            Self::Bandcamp => "Bandcamp",
            Self::Direct => "direct link",
            Self::Search => "search",
        }
    }

    /// Whether this service hands over only metadata, so the audio has to be
    /// found elsewhere. Worth stating in a UI: the file the user gets is not
    /// the service's own stream.
    pub fn is_catalogue_only(self) -> bool {
        matches!(self, Self::Spotify | Self::AppleMusic | Self::Deezer)
    }
}

/// What a link resolved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Track,
    Album,
    Playlist,
    /// An artist page: their most popular tracks, as far as the service lists
    /// them to an anonymous visitor.
    Artist,
    Search,
}

/// Where a track's audio comes from.
///
/// Carried on the track (and stored on the job row) because analysis is
/// stateless: a retry after a restart has to reproduce the same decision, and
/// the stream URL itself is usually signed and expired by then — so this
/// records the *identity* of the stream, not its URL, and the manager
/// re-resolves at download time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum AudioSource {
    /// Look the track up on YouTube Music / YouTube by its metadata.
    Match,
    /// A known YouTube video; its audio-only stream is used.
    #[serde(rename = "youtube")]
    YouTube { video_id: String },
    /// A URL the engine can fetch as a file or an HLS playlist.
    Direct {
        url: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        headers: Headers,
        /// The container the bytes arrive in, when known (`mp3`, `m4a`, …).
        container: Option<String>,
    },
    /// A SoundCloud track: transcodings are listed and a stream URL minted
    /// at download time, since they expire within minutes.
    #[serde(rename = "soundcloud")]
    SoundCloud { track_id: u64 },
    /// A Bandcamp track: the page is re-read at download time for the same
    /// reason.
    Bandcamp { page_url: String, track_id: u64 },
}

/// One track, as the catalogue describes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicTrack {
    /// Stable within its service: `spotify:track:…`, `yt:<videoId>`,
    /// `apple:<adamId>`. Used by a UI to select, never shown.
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub track_total: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<u32>,
    /// Full release date when known (`2001-03-12`), for the `date` tag.
    pub date: Option<String>,
    pub genre: Option<String>,
    /// Seconds.
    pub duration: Option<f64>,
    /// The largest cover the service offers.
    pub cover_url: Option<String>,
    pub isrc: Option<String>,
    /// The track's own page — what "open source" in a UI points at, and the
    /// URL written into the file's comment tag.
    pub source_url: String,
    pub service: Service,
    pub source: AudioSource,
    #[serde(default)]
    pub explicit: bool,
    /// Why the service says it cannot be played (region-locked, removed), when
    /// it says so. Still offered: a matched download may well work anyway.
    pub unavailable: Option<String>,
}

impl MusicTrack {
    /// `Artist A, Artist B` — how a tag and a filename both want the artists.
    pub fn artist_line(&self) -> String {
        self.artists.join(", ")
    }

    /// The first artist, for sorting and folder names.
    pub fn primary_artist(&self) -> Option<&str> {
        self.artists.first().map(String::as_str)
    }
}

/// A resolved link: one track, or many.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicSource {
    pub kind: SourceKind,
    pub service: Service,
    /// The playlist/album name, or the track title for a single.
    pub title: String,
    /// Artist, curator, or channel.
    pub owner: Option<String>,
    pub cover_url: Option<String>,
    pub source_url: String,
    pub tracks: Vec<MusicTrack>,
    /// How many tracks the service *says* the collection holds, when that is
    /// more than could be listed. Shown so a truncated playlist is never
    /// silently short.
    pub total: Option<u32>,
    /// Anything the user should know about the listing — a cap the service
    /// imposes on anonymous readers, most often.
    pub note: Option<String>,
}

/// The file the user wants at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioFormat {
    /// Keep whatever the source serves, in a clean container: `m4a` for AAC,
    /// `opus` for Opus, `mp3` for MP3. No re-encode, no quality lost.
    #[default]
    Best,
    Mp3,
    M4a,
    Opus,
    Ogg,
    /// Lossless *container*. Encoding a lossy stream to FLAC preserves it
    /// exactly, but does not restore anything the source already lost — every
    /// stream these services serve is lossy, and a UI should say so.
    Flac,
    Wav,
    Alac,
}

impl AudioFormat {
    pub const ALL: [AudioFormat; 8] = [
        Self::Best,
        Self::Mp3,
        Self::M4a,
        Self::Opus,
        Self::Ogg,
        Self::Flac,
        Self::Wav,
        Self::Alac,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Best => "best",
            Self::Mp3 => "mp3",
            Self::M4a => "m4a",
            Self::Opus => "opus",
            Self::Ogg => "ogg",
            Self::Flac => "flac",
            Self::Wav => "wav",
            Self::Alac => "alac",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.as_str() == raw.trim().to_ascii_lowercase())
    }

    /// The file extension, or `None` for `Best`, whose extension is only
    /// known once the source has been fetched.
    pub fn extension(self) -> Option<&'static str> {
        match self {
            Self::Best => None,
            Self::Mp3 => Some("mp3"),
            Self::M4a | Self::Alac => Some("m4a"),
            Self::Opus => Some("opus"),
            Self::Ogg => Some("ogg"),
            Self::Flac => Some("flac"),
            Self::Wav => Some("wav"),
        }
    }

    pub fn is_lossless(self) -> bool {
        matches!(self, Self::Flac | Self::Wav | Self::Alac)
    }

    /// Bitrate (kbps) used when the user did not choose one. Lossless formats
    /// and `Best` have none.
    pub fn default_bitrate(self) -> Option<u32> {
        match self {
            Self::Mp3 => Some(320),
            Self::M4a => Some(256),
            Self::Opus => Some(160),
            Self::Ogg => Some(192),
            _ => None,
        }
    }

    /// The bitrates a picker should offer for this format.
    pub fn bitrate_choices(self) -> &'static [u32] {
        match self {
            Self::Mp3 => &[128, 192, 256, 320],
            Self::M4a => &[128, 192, 256],
            Self::Opus => &[96, 128, 160, 192],
            Self::Ogg => &[128, 192, 256],
            _ => &[],
        }
    }
}

/// Where a collection's files go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FolderMode {
    /// A folder named after the playlist or album, inside the music folder.
    #[default]
    Collection,
    /// A folder the user named.
    Custom,
    /// Straight into the music folder.
    None,
}

/// Everything about the *output* the user can choose. Captured on every job
/// at enqueue, so changing the defaults never rewrites queued work.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OutputOptions {
    pub format: AudioFormat,
    /// For lossy re-encodes. `None` means the format's default.
    pub bitrate_kbps: Option<u32>,
    /// The music folder. `None` means the engine's default music folder.
    pub directory: Option<PathBuf>,
    pub folder_mode: FolderMode,
    /// Used with [`FolderMode::Custom`].
    pub folder_name: Option<String>,
    /// See [`name::render`] for the placeholders.
    pub filename_template: String,
    /// Prefix `01 - ` to files that are part of a collection.
    pub number_tracks: bool,
    pub embed_metadata: bool,
    pub embed_cover: bool,
    /// Look cover art up on Apple Music and Deezer when the source has none,
    /// or when all it has is a video still. A matched track otherwise carries
    /// YouTube's 16:9 thumbnail, which every player would then show as the
    /// album cover for that song.
    pub fetch_artwork: bool,
    /// Write the cover beside the files: `cover.jpg` (and `folder.jpg`, which
    /// is the name Windows reads for a folder thumbnail) for a collection, and
    /// a picture next to a single track.
    pub save_cover_file: bool,
    /// Look the lyrics up on lrclib.net; embed them and write a `.lrc` when
    /// they are time-synced.
    pub fetch_lyrics: bool,
    /// Write an `.m3u8` for a collection listing its files in order.
    pub write_playlist_file: bool,
    /// Skip a track whose file already exists, rather than numbering a copy.
    pub skip_existing: bool,
    /// EBU R128 loudness normalisation (`loudnorm`). Forces a re-encode.
    pub normalize_loudness: bool,
}

impl Default for OutputOptions {
    fn default() -> Self {
        Self {
            format: AudioFormat::Best,
            bitrate_kbps: None,
            directory: None,
            folder_mode: FolderMode::Collection,
            folder_name: None,
            filename_template: name::DEFAULT_TEMPLATE.into(),
            number_tracks: true,
            embed_metadata: true,
            embed_cover: true,
            fetch_artwork: true,
            save_cover_file: false,
            fetch_lyrics: false,
            write_playlist_file: false,
            skip_existing: true,
            normalize_loudness: false,
        }
    }
}

impl OutputOptions {
    /// The bitrate a re-encode should use, in kbps, when the format is lossy.
    pub fn bitrate(&self) -> Option<u32> {
        match self.format {
            f if f.bitrate_choices().is_empty() => None,
            f => Some(self.bitrate_kbps.filter(|b| *b > 0).unwrap_or_else(|| f.default_bitrate().unwrap_or(192))),
        }
    }
}

/// Music-wide settings: the output defaults and how the worker runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MusicSettings {
    pub output: OutputOptions,
    /// Tracks fetched and converted at once.
    pub max_concurrent: usize,
    /// Search YouTube Music before plain YouTube when matching a catalogue
    /// track. YouTube Music's results are songs, not videos, so the match is
    /// nearly always the studio recording; plain YouTube is the fallback.
    pub prefer_youtube_music: bool,
}

impl Default for MusicSettings {
    fn default() -> Self {
        Self {
            output: OutputOptions::default(),
            max_concurrent: 3,
            prefer_youtube_music: true,
        }
    }
}

// ─── Shared fetch helpers ────────────────────────────────────────────────────

/// A current desktop browser, for the catalogue pages that serve a JS shell
/// (or nothing) to an unfamiliar client.
pub(crate) const BROWSER_UA: &str = crate::video::youtube::WEB_USER_AGENT;

/// GET a JSON document.
pub(crate) async fn get_json(client: &Client, url: &str) -> Result<serde_json::Value> {
    let response = client
        .get(url)
        .header("User-Agent", BROWSER_UA)
        .header("Accept", "application/json, text/plain, */*")
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::UnexpectedStatus { status: status.as_u16(), url: url.to_string() });
    }
    let text = response.text().await?;
    serde_json::from_str(&text).map_err(|e| Error::InvalidInput(format!("{url} did not answer with JSON: {e}")))
}

/// GET an HTML page as a browser would.
pub(crate) async fn get_html(client: &Client, url: &str) -> Result<String> {
    let headers: Headers = vec![("User-Agent".into(), BROWSER_UA.into())];
    crate::video::net::fetch_text(client, url, &headers).await
}

/// Where a short link lands, following redirects. The URL itself when it
/// does not redirect.
pub(crate) async fn resolve_redirects(client: &Client, url: &str) -> Result<String> {
    let response = client.get(url).header("User-Agent", BROWSER_UA).send().await?;
    if !response.status().is_success() {
        return Err(Error::UnexpectedStatus { status: response.status().as_u16(), url: "music share link".into() });
    }
    Ok(response.url().to_string())
}

/// The JSON inside `<script id="…">…</script>`, for the pages that ship
/// their data that way.
pub(crate) fn script_json(html: &str, id_attr: &str) -> Option<serde_json::Value> {
    let marker = format!("id=\"{id_attr}\"");
    let at = html.find(&marker)?;
    let open = html[at..].find('>')? + at + 1;
    let close = html[open..].find("</script>")? + open;
    serde_json::from_str(html[open..close].trim()).ok()
}

/// A string at a JSON pointer, trimmed, or `None` when absent or empty.
pub(crate) fn json_str(v: &serde_json::Value, pointer: &str) -> Option<String> {
    v.pointer(pointer)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The year in an ISO date (`2001-03-12`, `1987-11-12T00:00:00Z`).
pub(crate) fn year_of(date: &str) -> Option<u32> {
    date.get(..4).and_then(|y| y.parse().ok()).filter(|y| *y > 1000)
}

/// `mm:ss` or `h:mm:ss` into seconds.
pub(crate) fn parse_clock(text: &str) -> Option<f64> {
    let mut total = 0f64;
    for part in text.trim().split(':') {
        let n: f64 = part.trim().parse().ok()?;
        total = total * 60.0 + n;
    }
    (total > 0.0).then_some(total)
}

// ─── Link classification ─────────────────────────────────────────────────────

fn host_of(url: &str) -> String {
    reqwest::Url::parse(url.trim()).ok().and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_default().trim_start_matches("www.").to_ascii_lowercase()
}

/// Which service a link belongs to, if it is a link this module handles.
///
/// A plain YouTube watch link counts: a user pasting one into the Music view
/// wants its audio, and the extractor already exists.
pub fn service_for(url: &str) -> Option<Service> {
    let trimmed = url.trim();
    if trimmed.starts_with("spotify:") {
        return Some(Service::Spotify);
    }
    if !reqwest::Url::parse(trimmed).is_ok_and(|url| matches!(url.scheme(), "http" | "https")) {
        return None;
    }
    let host = host_of(trimmed);
    let is = |h: &str| host == h || host.ends_with(&format!(".{h}"));

    if is("spotify.com") || is("spotify.link") {
        Some(Service::Spotify)
    } else if is("music.apple.com") || is("itunes.apple.com") || is("geo.music.apple.com") {
        Some(Service::AppleMusic)
    } else if is("deezer.com") || is("deezer.page.link") || is("link.deezer.com") {
        Some(Service::Deezer)
    } else if is("music.youtube.com") {
        Some(Service::YouTubeMusic)
    } else if is("youtube.com") || is("youtu.be") || is("youtube-nocookie.com") {
        Some(Service::YouTube)
    } else if is("soundcloud.com") || is("snd.sc") || is("soundcloud.app.goo.gl") {
        Some(Service::SoundCloud)
    } else if is("bandcamp.com") {
        Some(Service::Bandcamp)
    } else if direct::is_audio_url(trimmed) {
        Some(Service::Direct)
    } else {
        None
    }
}

/// Whether the input is a link at all, as opposed to a search query.
pub fn looks_like_url(input: &str) -> bool {
    let t = input.trim();
    t.starts_with("spotify:") || reqwest::Url::parse(t).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

/// Resolve a link into the tracks behind it.
///
/// `headers` is the caller's session for the link's site, when it has one; the
/// catalogue lookups here need none, but a YouTube link's audio does benefit
/// from a signed-in session and the manager attaches it at download time.
pub async fn analyze(client: &Client, url: &str, headers: &Headers) -> Result<MusicSource> {
    let url = url.trim();
    if !looks_like_url(url) {
        return Err(Error::InvalidInput(
            "this is not a link — paste a Spotify, Apple Music, Deezer, YouTube, SoundCloud or Bandcamp URL, or search by name".into(),
        ));
    }
    match service_for(url) {
        Some(Service::Spotify) => spotify::analyze(client, url).await,
        Some(Service::AppleMusic) => apple::analyze(client, url).await,
        Some(Service::Deezer) => deezer::analyze(client, url).await,
        Some(Service::YouTube) | Some(Service::YouTubeMusic) => ytmusic::analyze(client, url, headers).await,
        Some(Service::SoundCloud) => soundcloud::analyze(client, url).await,
        Some(Service::Bandcamp) => bandcamp::analyze(client, url).await,
        Some(Service::Direct) => direct::analyze(client, url, headers).await,
        Some(Service::Search) | None => direct::analyze(client, url, headers).await,
    }
}

/// Search by name. The result is a [`SourceKind::Search`] source whose tracks
/// are the top matches, best first, each already bound to a YouTube video.
pub async fn search(client: &Client, query: &str, prefer_music: bool) -> Result<MusicSource> {
    let query = query.trim();
    if query.is_empty() {
        return Err(Error::InvalidInput("enter something to search for".into()));
    }
    ytmusic::search_source(client, query, prefer_music).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn share_redirects_keep_the_destination_and_reject_error_pages() {
        use axum::response::IntoResponse;
        let app=axum::Router::new().fallback(|uri:axum::http::Uri|async move{
            match uri.path(){
                "/short"=>axum::response::Redirect::temporary("/track?token=required").into_response(),
                "/track"=>"track page".into_response(),
                _=>axum::http::StatusCode::NOT_FOUND.into_response(),
            }
        });
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base=format!("http://{}",listener.local_addr().unwrap());
        let server=tokio::spawn(async move{axum::serve(listener,app).await.unwrap();});
        let client=Client::new();
        assert_eq!(resolve_redirects(&client,&format!("{base}/short")).await.unwrap(),format!("{base}/track?token=required"));
        assert!(matches!(resolve_redirects(&client,&format!("{base}/missing")).await,Err(Error::UnexpectedStatus{status:404,..})));
        server.abort();
    }

    #[test]
    fn links_are_classified_by_host_not_substring() {
        assert_eq!(service_for("https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC"), Some(Service::Spotify));
        assert_eq!(service_for("spotify:track:4uLU6hMCjMI75M1A2tKUQC"), Some(Service::Spotify));
        assert_eq!(service_for("https://music.apple.com/us/album/x/1?i=2"), Some(Service::AppleMusic));
        assert_eq!(service_for("https://www.deezer.com/track/3135556"), Some(Service::Deezer));
        assert_eq!(service_for("https://music.youtube.com/watch?v=x"), Some(Service::YouTubeMusic));
        assert_eq!(service_for("https://youtu.be/x"), Some(Service::YouTube));
        assert_eq!(service_for("https://soundcloud.com/forss/flickermood"), Some(Service::SoundCloud));
        assert_eq!(service_for("https://on.soundcloud.com/oUKDDayY7aSAqt1WPM"), Some(Service::SoundCloud));
        assert_eq!(service_for("HTTPS://SOUNDCLOUD.COM:443/artist/track"), Some(Service::SoundCloud));
        assert_eq!(service_for("https://soundcloud.com@evil.example/song"), None);
        assert_eq!(service_for("https://snd.sc/short"), Some(Service::SoundCloud));
        assert_eq!(service_for("https://spotify.link/short"), Some(Service::Spotify));
        assert_eq!(service_for("https://deezer.page.link/short"), Some(Service::Deezer));
        assert_eq!(service_for("https://cloudkicker.bandcamp.com/album/beacons"), Some(Service::Bandcamp));
        assert_eq!(service_for("https://cdn.example/song.flac?x=1"), Some(Service::Direct));
        assert_eq!(service_for("https://spotify.com.evil.example/track/x"), None);
        assert_eq!(service_for("https://example.com/page"), None);
        assert_eq!(service_for("daft punk one more time"), None);
    }

    #[test]
    fn services_and_sources_serialize_under_plain_names() {
        assert_eq!(serde_json::to_string(&Service::YouTubeMusic).unwrap(), "\"youtube_music\"");
        assert_eq!(serde_json::to_string(&Service::SoundCloud).unwrap(), "\"soundcloud\"");
        let src = AudioSource::YouTube { video_id: "x".into() };
        assert_eq!(serde_json::to_string(&src).unwrap(), r#"{"kind":"youtube","videoId":"x"}"#);
        let back: AudioSource = serde_json::from_str(r#"{"kind":"soundcloud","trackId":293}"#).unwrap();
        assert_eq!(back, AudioSource::SoundCloud { track_id: 293 });
    }

    #[test]
    fn formats_round_trip_and_know_their_extensions() {
        for f in AudioFormat::ALL {
            assert_eq!(AudioFormat::parse(f.as_str()), Some(f));
        }
        assert_eq!(AudioFormat::Best.extension(), None);
        assert_eq!(AudioFormat::Alac.extension(), Some("m4a"));
        assert!(AudioFormat::Flac.is_lossless());
        assert!(!AudioFormat::Mp3.is_lossless());
    }

    #[test]
    fn bitrate_falls_back_to_the_formats_default() {
        let mut o = OutputOptions { format: AudioFormat::Mp3, ..Default::default() };
        assert_eq!(o.bitrate(), Some(320));
        o.bitrate_kbps = Some(192);
        assert_eq!(o.bitrate(), Some(192));
        o.format = AudioFormat::Flac;
        assert_eq!(o.bitrate(), None, "lossless has no bitrate");
    }

    #[test]
    fn settings_written_by_an_older_build_still_load() {
        let s: MusicSettings = serde_json::from_str(r#"{"maxConcurrent":2}"#).unwrap();
        assert_eq!(s.max_concurrent, 2);
        assert_eq!(s.output, OutputOptions::default());
    }
}
