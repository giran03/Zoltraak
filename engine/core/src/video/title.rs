use std::sync::LazyLock;

/// Extracts a clean, human-readable website name from a URL.
/// E.g.
/// - "https://www.tiktok.com/@user/video/123" -> "tiktok"
/// - "https://www.youtube.com/watch?v=123" -> "youtube"
/// - "https://youtu.be/123" -> "youtube"
/// - "https://m.facebook.com/watch/123" -> "facebook"
/// - "https://fb.watch/123" -> "facebook"
/// - "https://www.instagram.com/reel/123" -> "instagram"
/// - "https://twitter.com/user/status/123" -> "twitter"
/// - "https://x.com/user/status/123" -> "x"
/// - "https://v.redd.it/123" -> "reddit"
/// - "https://www.reddit.com/r/test/123" -> "reddit"
/// - "https://vimeo.com/123" -> "vimeo"
/// - "https://sub.example.com/video" -> "example"
pub fn extract_website_name(url: &str) -> String {
    let clean = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("//");
    let host = clean
        .split(['/', '?', '#', ':'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();

    if host.is_empty() {
        return "website".to_string();
    }

    // Special platform aliases
    if host.contains("youtube.com") || host.contains("youtu.be") {
        return "youtube".to_string();
    }
    if host.contains("facebook.com") || host.contains("fb.watch") {
        return "facebook".to_string();
    }
    if host.contains("tiktok.com") {
        return "tiktok".to_string();
    }
    if host.contains("instagram.com") {
        return "instagram".to_string();
    }
    if host.contains("twitter.com") {
        return "twitter".to_string();
    }
    if host == "x.com" || host.ends_with(".x.com") {
        return "x".to_string();
    }
    if host.contains("reddit.com") || host.contains("v.redd.it") {
        return "reddit".to_string();
    }
    if host.contains("vimeo.com") {
        return "vimeo".to_string();
    }

    // Generic domain extraction: strip leading common subdomains
    let mut domain = host.as_str();
    for prefix in ["www.", "m.", "web.", "mobile.", "v.", "player.", "embed.", "app.", "cdn."] {
        if domain.starts_with(prefix) {
            domain = &domain[prefix.len()..];
        }
    }

    // Split parts and extract main domain label
    let parts: Vec<&str> = domain.split('.').collect();
    if parts.len() >= 2 {
        // Handle common 2-level TLDs like .co.uk, .com.au, .co.jp, etc.
        if parts.len() >= 3 && matches!(parts[parts.len() - 2], "co" | "com" | "org" | "net" | "gov" | "edu") {
            parts[parts.len() - 3].to_string()
        } else {
            parts[parts.len() - 2].to_string()
        }
    } else if !parts.is_empty() && !parts[0].is_empty() {
        parts[0].to_string()
    } else {
        "website".to_string()
    }
}

/// Generates a random 6-character alphanumeric string of numbers and letters.
pub fn random_alphanumeric(len: usize) -> String {
    let raw = uuid::Uuid::new_v4().simple().to_string();
    raw.chars().take(len).collect()
}

static GENERIC_TITLES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    vec![
        "video",
        "video.mp4",
        "watch",
        "play",
        "embed",
        "stream",
        "media",
        "index",
        "index.html",
        "untitled",
        "download",
        "zoltraak",
        // Site default generic titles
        "tiktok",
        "tiktok - make your day",
        "tiktok video",
        "instagram",
        "instagram video",
        "facebook",
        "facebook video",
        "facebook - log in or sign up",
        "youtube",
        "youtube video",
        "twitter",
        "twitter video",
        "x",
        "x video",
        "reddit",
        "reddit video",
        "vimeo",
        "vimeo video",
    ]
});

/// Returns `true` if the title is empty/missing, equals generic terms, or matches/defaults
/// to the source website's name.
pub fn is_missing_or_site_default_title(title: &str, url: &str) -> bool {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return true;
    }

    let lower = trimmed.to_ascii_lowercase();

    // Check generic title list
    if GENERIC_TITLES.iter().any(|&g| lower == g) {
        return true;
    }

    // Check if title matches extracted website name
    let site_name = extract_website_name(url);
    if lower == site_name {
        return true;
    }

    // Check if title matches domain/host
    let clean_url = url.trim_start_matches("https://").trim_start_matches("http://");
    let host = clean_url.split(['/', '?', '#', ':']).next().unwrap_or("").to_ascii_lowercase();
    if !host.is_empty() && (lower == host || lower == format!("www.{host}")) {
        return true;
    }

    // Check titles that are just "site_name video" or "watch site_name"
    if lower == format!("{site_name} video") || lower == format!("watch {site_name}") {
        return true;
    }

    // Check if title starts with the website's default slogan (e.g., "TikTok - Trends Start Here...")
    if lower.starts_with("tiktok - trends start here") || lower.starts_with("tiktok - make your day") {
        return true;
    }

    false
}

/// Ensures the video title is meaningful. If missing or defaulting to the website name/generic title,
/// formats it as `{website name}_{random string and numbers}`.
pub fn ensure_valid_title(title: &str, url: &str) -> String {
    let site_name = extract_website_name(url);
    if is_missing_or_site_default_title(title, url) {
        let rand_str = random_alphanumeric(6);
        format!("{site_name}_{rand_str}")
    } else {
        title.trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_website_name() {
        assert_eq!(extract_website_name("https://www.tiktok.com/@user/video/123"), "tiktok");
        assert_eq!(extract_website_name("https://www.youtube.com/watch?v=123"), "youtube");
        assert_eq!(extract_website_name("https://youtu.be/123"), "youtube");
        assert_eq!(extract_website_name("https://m.facebook.com/watch/123"), "facebook");
        assert_eq!(extract_website_name("https://fb.watch/123"), "facebook");
        assert_eq!(extract_website_name("https://www.instagram.com/reel/123"), "instagram");
        assert_eq!(extract_website_name("https://twitter.com/user/status/123"), "twitter");
        assert_eq!(extract_website_name("https://x.com/user/status/123"), "x");
        assert_eq!(extract_website_name("https://v.redd.it/123"), "reddit");
        assert_eq!(extract_website_name("https://vimeo.com/123"), "vimeo");
        assert_eq!(extract_website_name("https://sub.example.co.uk/page"), "example");
    }

    #[test]
    fn test_is_missing_or_site_default_title() {
        assert!(is_missing_or_site_default_title("", "https://www.tiktok.com/123"));
        assert!(is_missing_or_site_default_title("TikTok", "https://www.tiktok.com/123"));
        assert!(is_missing_or_site_default_title("TikTok - Make Your Day", "https://www.tiktok.com/123"));
        assert!(is_missing_or_site_default_title("Instagram video", "https://www.instagram.com/reel/123"));
        assert!(is_missing_or_site_default_title("YouTube", "https://www.youtube.com/watch?v=123"));
        assert!(is_missing_or_site_default_title("video", "https://example.com/stream"));
        assert!(!is_missing_or_site_default_title("Awesome Cat Video", "https://www.tiktok.com/123"));
    }

    #[test]
    fn test_ensure_valid_title() {
        let title1 = ensure_valid_title("TikTok", "https://www.tiktok.com/@user/video/123");
        assert!(title1.starts_with("tiktok_"));
        assert_eq!(title1.len(), "tiktok_".len() + 6);

        let title2 = ensure_valid_title("", "https://www.youtube.com/watch?v=123");
        assert!(title2.starts_with("youtube_"));
        assert_eq!(title2.len(), "youtube_".len() + 6);

        let title3 = ensure_valid_title("My Custom Title", "https://www.tiktok.com/@user/video/123");
        assert_eq!(title3, "My Custom Title");
    }
}
