//! Filenames for tracks.
//!
//! A catalogue track has a real name; a stream URL does not. The template
//! turns the former into a filename the user recognises in a file manager,
//! with the track number keeping an album in order and the sanitiser keeping
//! Windows happy.

use super::MusicTrack;

/// `{artist} - {title}`: the convention most libraries expect.
pub const DEFAULT_TEMPLATE: &str = "{artist} - {title}";

/// The placeholders a template may use, for a settings screen to list.
pub const PLACEHOLDERS: &[(&str, &str)] = &[
    ("{title}", "track title"),
    ("{artist}", "first artist"),
    ("{artists}", "all artists, comma-separated"),
    ("{album}", "album name"),
    ("{album_artist}", "album artist"),
    ("{track}", "track number"),
    ("{track:02}", "track number, zero-padded"),
    ("{disc}", "disc number"),
    ("{year}", "release year"),
];

/// Render `template` for `track`, as a file *stem* — no extension, already
/// sanitised for a filesystem.
///
/// An unknown placeholder is left in place rather than erased, so a typo in a
/// template is visible in the result instead of silently producing
/// `- Title.mp3`. Empty values collapse their separators: a track with no
/// album under `{album} - {title}` becomes `Title`, not ` - Title`.
pub fn render(template: &str, track: &MusicTrack, position: Option<u32>) -> String {
    let track_number = track.track_number.or(position);
    let value = |key: &str| -> Option<String> {
        match key {
            "title" => Some(track.title.clone()),
            "artist" => track.primary_artist().map(str::to_string),
            "artists" => Some(track.artist_line()).filter(|s| !s.is_empty()),
            "album" => track.album.clone(),
            "album_artist" => track.album_artist.clone().or_else(|| track.primary_artist().map(str::to_string)),
            "track" => track_number.map(|n| n.to_string()),
            "track:02" => track_number.map(|n| format!("{n:02}")),
            "track:03" => track_number.map(|n| format!("{n:03}")),
            "disc" => track.disc_number.map(|n| n.to_string()),
            "year" => track.year.map(|y| y.to_string()),
            _ => None,
        }
    };

    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let key = &after[..close];
                match value(key) {
                    Some(v) => out.push_str(&v),
                    None if is_known(key) => {}
                    None => {
                        out.push('{');
                        out.push_str(key);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);

    let stem = collapse_separators(&out);
    // A slash in a title is punctuation here, not a directory: the sanitiser
    // would otherwise keep only what follows the last one.
    let stem = stem.replace(['/', '\\'], "-");
    let stem = crate::probe::sanitize_filename(&stem);
    // The sanitiser's own fallback is "download", which is a fine name for a
    // file and a poor one for a song.
    let stem = if stem == "download" { crate::probe::sanitize_filename(&track.title) } else { stem };
    truncate(&stem, 180)
}

fn is_known(key: &str) -> bool {
    matches!(
        key,
        "title" | "artist" | "artists" | "album" | "album_artist" | "track" | "track:02" | "track:03" | "disc" | "year"
    )
}

/// Tidy what empty placeholders leave behind: doubled separators, a dangling
/// ` - ` at either end, runs of whitespace.
fn collapse_separators(s: &str) -> String {
    let mut out = s.replace(" -  - ", " - ").replace("  ", " ");
    loop {
        let before = out.len();
        out = out.replace(" - - ", " - ");
        if out.len() == before {
            break;
        }
    }
    let trimmed = out.trim().trim_start_matches("- ").trim_end_matches(" -").trim();
    trimmed.to_string()
}

/// Cut at a char boundary so a long title never produces a name the
/// filesystem refuses.
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].trim_end().to_string()
}

/// A folder name for a collection: the title, sanitised, or a fallback.
///
/// A title with nothing readable left in it after sanitising — punctuation
/// only, or empty — takes the fallback. A folder called `---` names nothing.
pub fn folder_name(title: &str, fallback: &str) -> String {
    let name = crate::probe::sanitize_filename(&title.trim().replace(['/', '\\'], "-"));
    let usable = name != "download" && name.chars().any(char::is_alphanumeric);
    let name = if usable { name } else { crate::probe::sanitize_filename(fallback) };
    truncate(&name, 120)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::{AudioSource, Service};

    fn track() -> MusicTrack {
        MusicTrack {
            id: "t".into(),
            title: "One More Time".into(),
            artists: vec!["Daft Punk".into(), "Romanthony".into()],
            album: Some("Discovery".into()),
            album_artist: None,
            track_number: Some(1),
            track_total: Some(14),
            disc_number: Some(1),
            year: Some(2001),
            date: None,
            genre: None,
            duration: Some(320.0),
            cover_url: None,
            isrc: None,
            source_url: "https://example/t".into(),
            service: Service::Spotify,
            source: AudioSource::Match,
            explicit: false,
            unavailable: None,
        }
    }

    #[test]
    fn the_default_template_reads_artist_dash_title() {
        assert_eq!(render(DEFAULT_TEMPLATE, &track(), None), "Daft Punk - One More Time");
    }

    #[test]
    fn every_placeholder_renders() {
        let t = track();
        assert_eq!(
            render("{track:02} {artists} - {title} ({album}, {year}) d{disc}", &t, None),
            "01 Daft Punk, Romanthony - One More Time (Discovery, 2001) d1"
        );
    }

    #[test]
    fn missing_values_collapse_their_separators() {
        let mut t = track();
        t.album = None;
        t.track_number = None;
        assert_eq!(render("{album} - {title}", &t, None), "One More Time");
        assert_eq!(render("{track:02} - {title}", &t, None), "One More Time");
        // A position stands in for a missing track number.
        assert_eq!(render("{track:02} - {title}", &t, Some(7)), "07 - One More Time");
    }

    #[test]
    fn unknown_placeholders_stay_visible() {
        assert_eq!(render("{titel} - {title}", &track(), None), "{titel} - One More Time");
    }

    #[test]
    fn filesystem_hostile_characters_are_replaced() {
        let mut t = track();
        t.title = "What / Is: Love? <3>".into();
        let name = render("{title}", &t, None);
        assert!(!name.contains('/') && !name.contains(':') && !name.contains('?'), "{name}");
        assert!(name.starts_with("What"), "a slash must not drop everything before it: {name}");
    }

    #[test]
    fn folder_names_fall_back_when_the_title_is_unusable() {
        assert_eq!(folder_name("Today's Hits", "playlist"), "Today's Hits");
        assert_eq!(folder_name("AC/DC Live", "playlist"), "AC-DC Live");
        assert_eq!(folder_name("///", "playlist"), "playlist");
    }
}
