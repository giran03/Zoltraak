//! Rule-based destination routing.
//!
//! A category matches a download by its guessed filename or its URL and
//! supplies the directory it should land in instead of wherever the caller's
//! fallback would have put it. This runs once, at enqueue time — a category
//! is a filing rule, not a live subscription, so editing or deleting one
//! later does not move files that already queued under the old rule.

use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CategoryKind {
    /// Comma-separated file extensions, case-insensitive, leading dots
    /// tolerated ("zip, .rar, 7z").
    Extension,
    /// A hostname or its subdomains ("github.com" also matches
    /// "raw.githubusercontent.com"... no — matches only exact subdomains of
    /// github.com, e.g. "gist.github.com").
    Domain,
    /// A regular expression matched against the full URL.
    Regex,
}

impl CategoryKind {
    fn as_str(self) -> &'static str {
        match self {
            CategoryKind::Extension => "extension",
            CategoryKind::Domain => "domain",
            CategoryKind::Regex => "regex",
        }
    }
}

impl std::fmt::Display for CategoryKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for CategoryKind {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "extension" => Ok(CategoryKind::Extension),
            "domain" => Ok(CategoryKind::Domain),
            "regex" => Ok(CategoryKind::Regex),
            other => Err(format!("unknown category kind {other:?}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Category {
    pub id: i64,
    pub name: String,
    pub kind: CategoryKind,
    pub pattern: String,
    pub directory: PathBuf,
    /// Higher runs first when more than one rule could match.
    pub priority: i64,
    pub enabled: bool,
}

/// A category to create. Validated in [`validate`] before it ever reaches
/// SQLite, so a bad regex fails the request instead of silently never
/// matching anything.
#[derive(Debug, Clone)]
pub struct NewCategory {
    pub name: String,
    pub kind: CategoryKind,
    pub pattern: String,
    pub directory: PathBuf,
    pub priority: i64,
}

/// Reject what would either match nothing or match everything by accident.
pub fn validate(kind: CategoryKind, pattern: &str) -> Result<()> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return Err(Error::InvalidInput("category pattern cannot be empty".into()));
    }
    if kind == CategoryKind::Regex {
        regex::Regex::new(trimmed)
            .map_err(|e| Error::InvalidInput(format!("invalid regex: {e}")))?;
    }
    Ok(())
}

impl Category {
    /// Whether this rule claims `url`/`filename`. Disabled rules never match.
    pub fn matches(&self, url: &str, filename: &str) -> bool {
        if !self.enabled {
            return false;
        }
        match self.kind {
            CategoryKind::Extension => extension_matches(&self.pattern, filename),
            CategoryKind::Domain => domain_matches(&self.pattern, url),
            CategoryKind::Regex => regex::Regex::new(&self.pattern)
                .map(|re| re.is_match(url))
                .unwrap_or(false),
        }
    }
}

fn extension_matches(pattern: &str, filename: &str) -> bool {
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext.is_empty() {
        return false;
    }
    pattern
        .split(',')
        .map(|p| p.trim().trim_start_matches('.').to_ascii_lowercase())
        .any(|p| p == ext)
}

fn domain_matches(pattern: &str, url: &str) -> bool {
    let Some(host) = host_of(url) else { return false };
    let pattern = pattern.trim().trim_start_matches('.').to_ascii_lowercase();
    if pattern.is_empty() {
        return false;
    }
    host == pattern || host.ends_with(&format!(".{pattern}"))
}

/// Extracts the host from an `http(s)://` URL without pulling in a full URL
/// parser for a comparison this simple.
fn host_of(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    // Userinfo (`user:pass@host`) is stripped rather than parsed: nothing
    // that runs against a category pattern needs it.
    let rest = rest.rsplit_once('@').map(|(_, host)| host).unwrap_or(rest);
    let end = rest.find(['/', '?', '#', ':']).unwrap_or(rest.len());
    let host = &rest[..end];
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn category(kind: CategoryKind, pattern: &str) -> Category {
        Category {
            id: 1,
            name: "test".into(),
            kind,
            pattern: pattern.into(),
            directory: PathBuf::from("/dest"),
            priority: 0,
            enabled: true,
        }
    }

    #[test]
    fn extension_matches_case_insensitively_and_ignores_leading_dots() {
        let cat = category(CategoryKind::Extension, "zip, .RAR, 7z");
        assert!(cat.matches("https://x.example/a.zip", "a.zip"));
        assert!(cat.matches("https://x.example/a.RAR", "a.RAR"));
        assert!(cat.matches("https://x.example/a.7z", "a.7z"));
        assert!(!cat.matches("https://x.example/a.iso", "a.iso"));
    }

    #[test]
    fn extension_with_no_dot_never_matches() {
        let cat = category(CategoryKind::Extension, "zip");
        assert!(!cat.matches("https://x.example/readme", "readme"));
    }

    #[test]
    fn domain_matches_the_exact_host_and_its_subdomains_only() {
        let cat = category(CategoryKind::Domain, "github.com");
        assert!(cat.matches("https://github.com/a/b", "b"));
        assert!(cat.matches("https://gist.github.com/a", "a"));
        // The reverse must not match — a rule for the parent should not
        // accidentally claim an unrelated domain that merely contains it.
        assert!(!cat.matches("https://notgithub.com/a", "a"));
        assert!(!cat.matches("https://evilgithub.com.attacker.net/a", "a"));
    }

    #[test]
    fn regex_matches_the_full_url() {
        let cat = category(CategoryKind::Regex, r"\.iso(\?|$)");
        assert!(cat.matches("https://x.example/ubuntu.iso", "ubuntu.iso"));
        assert!(!cat.matches("https://x.example/ubuntu.isoz", "ubuntu.isoz"));
    }

    #[test]
    fn a_disabled_category_never_matches() {
        let mut cat = category(CategoryKind::Extension, "zip");
        cat.enabled = false;
        assert!(!cat.matches("https://x.example/a.zip", "a.zip"));
    }

    #[test]
    fn validate_rejects_empty_patterns() {
        assert!(validate(CategoryKind::Extension, "   ").is_err());
    }

    #[test]
    fn validate_rejects_unparseable_regex() {
        assert!(validate(CategoryKind::Regex, "(unclosed").is_err());
        assert!(validate(CategoryKind::Regex, r"\.iso$").is_ok());
    }
}
