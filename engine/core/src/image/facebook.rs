//! Post-scoped Facebook media sets. No profile crawling or browser automation.
use std::{collections::{HashSet, VecDeque}, future::Future, sync::LazyLock, time::Duration};

use regex::Regex;
use reqwest::{Client, Url};
use serde_json::Value;

use super::{sites, Completeness, ImageAlbum, ImageItem};
use crate::{error::{Error, Result}, headers::{self, Headers}, video::site};

const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/124.0.0.0 Safari/537.36";
const SIGN_IN: &str = "Facebook requested sign-in or refused access. Refresh Facebook's shared sign-in from the extension, then extract again.";
const GALLERY_HELP: &str = "Open this post's photo gallery in Facebook, view the remaining photos, then use Download post with Zoltraak again.";

fn facebook_url(raw: &str) -> Option<Url> {
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    (url.scheme() == "https" && (host == "facebook.com" || host.ends_with(".facebook.com")))
        .then_some(url)
}

pub(crate) fn post_id(raw: &str) -> Option<String> {
    let url = facebook_url(raw)?;
    if let Some((_, id)) = url.query_pairs().find(|(k, _)| k == "story_fbid") {
        return Some(id.into_owned());
    }
    let parts: Vec<_> = url.path_segments()?.filter(|p| !p.is_empty()).collect();
    let at = parts.iter().position(|p| *p == "posts")?;
    parts.get(at + 1..)?.iter().rev().find(|p| p.starts_with("pfbid") || p.chars().all(|c| c.is_ascii_digit()))
        .map(|s| s.to_string())
}

fn documents(html: &str) -> Vec<Value> {
    static SCRIPT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<script\b[^>]*>(.*?)</script>").unwrap());
    SCRIPT.captures_iter(html).filter_map(|c| serde_json::from_str(c[1].trim()).ok()).collect()
}

// Walk structured payloads, never regex-match image URLs out of the entire page.
fn visit(value: &Value, f: &mut impl FnMut(&Value) -> bool) {
    if f(value) { return; }
    match value {
        Value::Object(map) => for (key, child) in map {
            if !matches!(key.as_str(), "feedback" | "comments" | "recommendations" | "related_stories") {
                visit(child, f);
            }
        },
        Value::Array(values) => for child in values { visit(child, f); },
        _ => {}
    }
}

fn identity(node: &Value) -> Option<&str> {
    node.get("id").and_then(Value::as_str)
}

#[derive(Default)]
struct Post {
    media: Vec<Value>,
    set: Option<String>,
    expected: Option<usize>,
    closed: bool,
}

fn collect_attachments(node: &Value, post: &mut Post) {
    visit(node, &mut |v| {
        if v.get("__typename").and_then(Value::as_str) == Some("Story") { return true; }
        for key in ["mediaset_token", "mediaSetToken", "mediasetToken"] {
            if let Some(set) = v.get(key).and_then(Value::as_str).filter(|s| s.starts_with("pcb.")) {
                post.set.get_or_insert_with(|| set.to_string());
            }
        }
        if matches!(v.get("__typename").and_then(Value::as_str), Some("Photo" | "Video")) {
            if identity(v).is_some() { post.media.push(v.clone()); }
            return true; // Do not collect a video's thumbnail as another attachment.
        }
        if let Some(connection) = v.get("all_subattachments") {
            if let Some(count) = connection.get("count").or_else(|| connection.get("total_count")).and_then(Value::as_u64) {
                post.expected = Some(post.expected.unwrap_or(0).max(count as usize));
            }
            post.closed |= connection.pointer("/page_info/has_next_page").and_then(Value::as_bool) == Some(false);
        }
        false
    });
}

fn parse_post(docs: &[Value], ids: &[String]) -> Post {
    let mut ids = ids.to_vec();
    // On pfbid permalinks the requested Relay node uses a numeric post_id,
    // while recommendations are separate data.node payloads. node_v2 is the
    // permalink's requested story, not a licence to take every Story in HTML.
    let mut requested = HashSet::new();
    for doc in docs {
        visit(doc, &mut |v| {
            if let Some(node) = v.pointer("/data/node_v2") {
                if node.get("__typename").and_then(Value::as_str) == Some("Story") {
                    if let Some(id) = node.get("post_id").and_then(Value::as_str) { requested.insert(id.to_string()); }
                }
            }
            false
        });
    }
    if !ids.is_empty() && requested.len() == 1 { ids.extend(requested); }
    let mut post = Post::default();
    for doc in docs {
        visit(doc, &mut |node| {
            let matches = ["post_id", "story_fbid", "id"].iter()
                .filter_map(|k| node.get(k).and_then(Value::as_str)).any(|id| ids.iter().any(|s| s == id))
                || ["url", "permalink_url"].iter().filter_map(|k| node.get(k).and_then(Value::as_str))
                    .filter_map(post_id).any(|id| ids.contains(&id));
            if !matches { return false; }
            // A matching Story may wrap attachments in comet_sections/content/story.
            // Stop at nested unrelated Stories and at attachment boundaries.
            visit(node, &mut |v| {
                if !std::ptr::eq(v, node) && v.get("__typename").and_then(Value::as_str) == Some("Story") {
                    if let Some(id) = v.get("post_id").and_then(Value::as_str) {
                        if !ids.iter().any(|s| s == id) { return true; }
                    }
                }
                for key in ["mediaset_token", "mediaSetToken", "mediasetToken"] {
                    if let Some(set) = v.get(key).and_then(Value::as_str).filter(|s| s.starts_with("pcb.")) {
                        post.set.get_or_insert_with(|| set.to_string());
                    }
                }
                if let Some(attachments) = v.get("attachments") {
                    collect_attachments(attachments, &mut post);
                    return true;
                }
                false
            });
            true
        });
    }
    // Photo links carry the post collection even when its token isn't a sibling.
    for media in &post.media {
        if let Some(url) = media.get("url").and_then(Value::as_str).and_then(facebook_url) {
            if let Some((_, set)) = url.query_pairs().find(|(k, v)| k == "set" && v.starts_with("pcb.")) {
                post.set.get_or_insert_with(|| set.into_owned());
            }
        }
    }
    if post.set.is_none() {
        post.set = ids.iter().find(|id| id.chars().all(|c| c.is_ascii_digit()))
            .map(|id| format!("pcb.{id}"));
    }
    post
}

/// Browser metadata is untrusted input. Keep only resolved, post-scoped CDN
/// media; raw page JSON, cookies and unrelated fields never enter the cache.
pub(crate) fn browser_album(url: &str, docs: &[Value]) -> Result<ImageAlbum> {
    let id = post_id(url).ok_or_else(|| Error::InvalidInput("browser attachments require a Facebook post permalink".into()))?;
    // Unlike a requested HTTP permalink response, arbitrary tab payloads must
    // explicitly identify this post; do not infer a node_v2 alias from a feed.
    let mut roots = Vec::new();
    for doc in docs {
        visit(doc, &mut |node| {
            let matches = ["post_id", "story_fbid"].iter().filter_map(|k| node.get(k).and_then(Value::as_str)).any(|s| s == id)
                || ["url", "permalink_url"].iter().filter_map(|k| node.get(k).and_then(Value::as_str)).filter_map(post_id).any(|s| s == id);
            if matches && node.get("attachments").is_some() { roots.push(node.clone()); return true; }
            false
        });
    }
    let post = parse_post(&roots, &[id]);
    let mut album = ImageAlbum { source_url: site::clean_facebook_url(url), title: "Facebook post".into(),
        expected_count: post.expected, completeness: Some(Completeness::Partial), ..Default::default() };
    for node in &post.media {
        if let Some(item) = media_item(node, &vec![]) {
            let urls = std::iter::once(item.url.as_str()).chain(item.thumbnail.as_deref())
                .chain(item.formats.iter().flat_map(|f| f.video_url.as_deref().into_iter().chain(f.audio_url.as_deref())));
            if urls.into_iter().all(facebook_cdn) { upsert(&mut album.items, item); }
        }
    }
    if album.items.is_empty() { return Err(Error::InvalidInput("no post-scoped Facebook attachments in browser data".into())); }
    if post.expected == Some(album.items.len()) { album.completeness = Some(Completeness::Complete); }
    else { album.warn(GALLERY_HELP); }
    Ok(album)
}

fn facebook_cdn(raw: &str) -> bool {
    Url::parse(raw).ok().is_some_and(|url| url.scheme() == "https" && url.username().is_empty() && url.password().is_none()
        && url.port_or_known_default() == Some(443) && url.host_str().is_some_and(|host|
            ["fbcdn.net", "fbsbx.com"].iter().any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))))
}

pub(crate) fn merge_album(album: &mut ImageAlbum, mut other: ImageAlbum) {
    if album.items.iter().chain(&other.items).any(|i| i.id.starts_with("facebook:")) {
        album.items.retain(|i| i.id.starts_with("facebook:"));
        other.items.retain(|i| i.id.starts_with("facebook:"));
    }
    let conflicting = matches!((album.expected_count, other.expected_count), (Some(a), Some(b)) if a != b)
        || album.warnings.iter().chain(&other.warnings).any(|s| s.contains("conflicting attachment counts"));
    let authoritative = other.completeness == Some(Completeness::Complete) && !conflicting;
    if authoritative {
        std::mem::swap(album, &mut other);
    }
    for item in other.items {
        if !authoritative || album.items.iter().any(|known| known.id == item.id && item.area() > known.area()) {
            upsert(&mut album.items, item);
        }
    }
    album.expected_count = match (album.expected_count, other.expected_count) {
        (Some(a), Some(b)) => Some(a.max(b)), (a, b) => a.or(b),
    };
    if (album.expected_count == Some(album.items.len()) || authoritative) && !conflicting {
        album.completeness = Some(Completeness::Complete);
        album.warnings.clear();
    } else {
        album.completeness = Some(Completeness::Partial);
        for warning in other.warnings { album.warn(warning); }
        if conflicting { album.warn("Facebook returned conflicting attachment counts. The full set could not be confirmed."); }
        album.warn(GALLERY_HELP);
    }
}

fn media_item(node: &Value, request_headers: &Headers) -> Option<ImageItem> {
    let id = identity(node)?;
    if !id.chars().all(|c| c.is_ascii_digit()) || id.is_empty() { return None; }
    if node.get("__typename").and_then(Value::as_str) == Some("Video") {
        // Restrict the existing format extractor to this attachment's JSON.
        let mut video = node.clone();
        if let Some(fields) = video.as_object_mut() {
            for key in ["creation_story", "feedback", "recommendations", "related_stories"] { fields.remove(key); }
        }
        let mut formats = site::facebook_formats(&video.to_string(), &format!("https://www.facebook.com/watch/?v={id}"));
        crate::video::sort_formats(&mut formats);
        for format in &mut formats {
            format.headers = download_headers(format.video_url.as_deref().unwrap_or(""), request_headers);
        }
        return ImageItem::video(format!("facebook:{id}"), formats).map(|item| item.with_thumbnail(
            node.pointer("/image/uri").and_then(Value::as_str).map(str::to_owned)));
    }
    let mut candidates = Vec::new();
    for key in ["viewer_image", "image", "photo_image", "full_size_image"] {
        if let Some(image) = node.get(key) {
            if let Some(uri) = image.get("uri").and_then(Value::as_str) {
                if !uri.starts_with("https://") { continue; }
                let width = image.get("width").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok());
                let height = image.get("height").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok());
                let mut item = ImageItem::new(format!("facebook:{id}"), uri).with_size(width, height);
                item.headers = download_headers(uri, request_headers);
                candidates.push(item);
            }
        }
    }
    candidates.into_iter().max_by_key(ImageItem::area)
}

fn download_headers(url: &str, request_headers: &Headers) -> Headers {
    let crawler = Url::parse(url).ok().is_some_and(|u| u.host_str() == Some("lookaside.fbsbx.com"));
    // Cookies stay in the existing site session vault, never in the result JSON.
    let ua = if crawler { sites::FB_CRAWLER_UA } else {
        request_headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("user-agent")).map(|(_, v)| v.as_str()).unwrap_or(BROWSER_UA)
    };
    let mut result = vec![("User-Agent".into(), ua.into())];
    if let Some((name, value)) = request_headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("referer")) {
        result.push((name.clone(), value.clone()));
    }
    result
}

pub(crate) fn browser_headers(album: &mut ImageAlbum, request_headers: &Headers) {
    for item in &mut album.items {
        item.headers = download_headers(&item.url, request_headers);
        for format in &mut item.formats {
            if let Some(url) = &format.video_url { format.headers = download_headers(url, request_headers); }
        }
    }
}

fn upsert(items: &mut Vec<ImageItem>, item: ImageItem) {
    if let Some(existing) = items.iter_mut().find(|i| i.id == item.id) {
        if item.area() >= existing.area() { *existing = item; }
    } else { items.push(item); }
}

struct Page { url: String, html: String }

async fn fetch_page(client: &Client, url: &str, request_headers: &Headers) -> std::result::Result<Page, String> {
    if facebook_url(url).is_none() { return Err("Facebook returned an unrelated navigation URL.".into()); }
    let request = client.get(url).timeout(Duration::from_secs(30))
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Sec-Fetch-Dest", "document").header("Sec-Fetch-Mode", "navigate")
        .header("Sec-Fetch-Site", "none").header("Sec-Fetch-User", "?1")
        .header("Upgrade-Insecure-Requests", "1").header("Cache-Control", "no-cache");
    let response = headers::apply(request, request_headers).send().await
        .map_err(|_| "Facebook could not be reached or the request timed out. Extract again to retry.".to_string())?;
    if !response.status().is_success() {
        return Err(match response.status().as_u16() {
            429 => "Facebook rate-limited extraction. Wait a while, then extract again.".into(),
            400 | 401 | 403 => SIGN_IN.into(),
            _ => "Facebook could not return an attachment. Extract again to retry.".into(),
        });
    }
    let final_url = response.url().to_string();
    if facebook_url(&final_url).is_none() || response.url().path().starts_with("/login")
        || response.url().path().starts_with("/checkpoint") { return Err(SIGN_IN.into()); }
    let html = response.text().await.map_err(|_| "Facebook's response was interrupted. Extract again to retry.".to_string())?;
    Ok(Page { url: site::clean_facebook_url(&final_url), html })
}

#[derive(Default)]
struct Slide { media: Option<Value>, members: Vec<Value>, next: Option<String>, previous: Option<String>, next_video: bool, terminal: bool, scoped: bool, foreign_next: bool }

fn parse_slide(docs: &[Value], id: &str, set: &str) -> Slide {
    let mut slide = Slide::default();
    let mut next_ids = HashSet::new();
    let mut null_next = false;
    for doc in docs {
        visit(doc, &mut |v| {
            if identity(v) == Some(id) && matches!(v.get("__typename").and_then(Value::as_str), Some("Photo" | "Video")) {
                // A navigation stub has only a type/id; prefer the full object.
                if slide.media.as_ref().is_none_or(|old| v.to_string().len() > old.to_string().len()) {
                    slide.media = Some(v.clone());
                }
            }
            if ["mediaset_token", "mediaSetToken", "mediasetToken"].iter()
                .any(|key| v.get(key).and_then(Value::as_str) == Some(set)) {
                slide.scoped = true;
                for key in ["all_subattachments", "media", "photos"] {
                    if let Some(connection) = v.get(key).filter(|n| n.get("nodes").is_some() || n.get("edges").is_some()) {
                        let mut collected = Post::default();
                        collect_attachments(connection, &mut collected);
                        slide.members.extend(collected.media);
                    }
                }
            }
            if let Some(url) = v.get("url").and_then(Value::as_str).and_then(facebook_url) {
                if url.query_pairs().any(|(k, val)| k == "set" && val == set)
                    && url.query_pairs().any(|(k, val)| (k == "fbid" || k == "v") && val == id) { slide.scoped = true; }
            }
            let owns_navigation = identity(v) == Some(id)
                || v.get("media").and_then(identity) == Some(id)
                || ["mediaset_token", "mediaSetToken", "mediasetToken"].iter()
                    .any(|key| !set.is_empty() && v.get(key).and_then(Value::as_str) == Some(set));
            if v.get("nextMediaAfterNodeId").is_some() && !owns_navigation { slide.foreign_next = true; }
            if let Some(next) = v.get("nextMediaAfterNodeId").filter(|_| owns_navigation) {
                if next.is_null() { null_next = true; }
                else if let Some(id) = identity(next) {
                    next_ids.insert(id.to_string());
                    slide.next_video = next.get("__typename").and_then(Value::as_str) == Some("Video");
                }
            }
            if owns_navigation {
                slide.previous = v.get("prevMediaBeforeNodeId").and_then(identity).map(str::to_string).or(slide.previous.take());
            }
            false
        });
    }
    slide.terminal = null_next && next_ids.is_empty() && !slide.foreign_next;
    if next_ids.len() == 1 && !null_next { slide.next = next_ids.into_iter().next(); }
    slide
}

fn canonical(page: &Page) -> String {
    static OG_URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<meta\s+property=["']og:url["']\s+content=["']([^"']+)"#).unwrap());
    OG_URL.captures(&page.html).map(|c| c[1].replace("&amp;", "&"))
        .filter(|url| post_id(url).is_some()).unwrap_or_else(|| page.url.clone())
}

async fn extract<F, Fut>(page: Page, request_headers: &Headers, fetch: &mut F) -> ImageAlbum
where
    F: FnMut(String, Headers) -> Fut,
    Fut: Future<Output = std::result::Result<Page, String>>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    let source_url = canonical(&page);
    let ids: Vec<_> = [post_id(&page.url), post_id(&source_url)].into_iter().flatten().collect();
    let post = parse_post(&documents(&page.html), &ids);
    let mut album = ImageAlbum {
        title: site::page_title(&page.html, &source_url), source_url,
        expected_count: post.expected, completeness: Some(Completeness::Unknown), ..Default::default()
    };
    for node in &post.media {
        if let Some(item) = media_item(node, request_headers) { upsert(&mut album.items, item); }
    }
    let unique: HashSet<_> = post.media.iter().filter_map(identity).collect();
    let listed_all = post.closed || post.expected.is_some_and(|n| unique.len() == n);
    // Some mixed posts embed only a video's ID and poster. Resolve that
    // attachment directly even when the other attachments are all photos.
    let mut resolved = HashSet::new();
    for node in &post.media {
        let Some(id) = identity(node) else { continue };
        if node.get("__typename").and_then(Value::as_str) != Some("Video")
            || album.items.iter().any(|i| i.id == format!("facebook:{id}"))
            || !resolved.insert(id.to_string()) { continue; }
        let mut url = Url::parse("https://www.facebook.com/watch/").unwrap();
        url.query_pairs_mut().append_pair("v", id);
        if let Ok(Ok(page)) = tokio::time::timeout_at(deadline, fetch(url.to_string(), request_headers.clone())).await {
            let slide = parse_slide(&documents(&page.html), id, "");
            if let Some(item) = slide.media.as_ref().and_then(|v| media_item(v, request_headers)) {
                upsert(&mut album.items, item);
            }
        }
    }
    // Resolution happens independently, but display and filenames follow the
    // original attachment order rather than putting late-resolved videos last.
    album.items.sort_by_key(|item| post.media.iter().position(|n|
        identity(n).is_some_and(|id| item.id == format!("facebook:{id}"))).unwrap_or(usize::MAX));
    if listed_all && unique.len() == album.items.len() && !unique.is_empty()
        && post.expected.is_none_or(|count| count == unique.len()) {
        album.completeness = Some(Completeness::Complete);
        return album;
    }
    let mut ended = false;
    if let Some(set) = post.set.as_deref().filter(|_| !post.media.is_empty()) {
        let mut pending: VecDeque<_> = post.media.iter().filter_map(|m| identity(m).map(|id|
            (id.to_string(), m.get("__typename").and_then(Value::as_str) == Some("Video")))).collect();
        let mut visited = HashSet::new();
        let mut ordered = album.items.clone();
        // Bounded per extraction, not an arbitrary ceiling on returned media.
        while let Some((id, next_video)) = pending.pop_front() {
            if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) { album.warn("Facebook returned an invalid attachment identifier."); break; }
            if !visited.insert(id.clone()) { continue; }
            let mut url = Url::parse(if next_video { "https://www.facebook.com/watch/" } else { "https://www.facebook.com/photo/" }).unwrap();
            url.query_pairs_mut().append_pair(if next_video { "v" } else { "fbid" }, &id).append_pair("set", set);
            let fetched = tokio::time::timeout_at(deadline, fetch(url.to_string(), request_headers.clone())).await;
            let page = match fetched {
                Ok(Ok(page)) => page,
                Ok(Err(reason)) => {
                    let refused = reason == SIGN_IN || reason.contains("rate-limit");
                    album.warn(reason);
                    if refused { break; }
                    continue;
                }
                Err(_) => { album.warn("Facebook extraction reached its time limit. Extract again to retry."); break; }
            };
            let slide = parse_slide(&documents(&page.html), &id, set);
            if !slide.scoped { album.warn("Facebook did not confirm that the next attachment belongs to this post."); continue; }
            if let Some(item) = slide.media.as_ref().and_then(|n| media_item(n, request_headers)) {
                upsert(&mut ordered, item);
            } else { album.warn("One attachment could not be resolved to an image or playable video."); }
            for node in &slide.members {
                if let Some(item) = media_item(node, request_headers) { upsert(&mut ordered, item); }
            }
            // Facebook's viewer wraps back to the first slide. Reaching the
            // post's authoritative count is completion, not a navigation loop.
            if album.expected_count.is_some_and(|count| ordered.len() == count) {
                ended = true;
                break;
            }
            // A terminal slide alone does not prove there were no holes earlier.
            if slide.terminal && !album.expected_count.is_some_and(|n| n > ordered.len()) { ended = true; }
            if let Some(next) = slide.next {
                if visited.contains(&next) { album.warn("Facebook repeated an attachment before the full set could be confirmed."); }
                else { pending.push_front((next, slide.next_video)); }
            } else if !slide.terminal { album.warn("Facebook did not expose the next attachment. The result may be incomplete."); }
            if let Some(previous) = slide.previous.filter(|id| !visited.contains(id)) { pending.push_back((previous, false)); }
        }
        album.items = ordered;
    }
    if album.items.is_empty() && post.media.is_empty() {
        album.items = sites::og_items(&page.html);
        for item in &mut album.items { item.headers = download_headers(&item.url, request_headers); }
    }
    let count_matches = album.expected_count.is_some_and(|n| n == album.items.len());
    let count_conflicts = album.expected_count.is_some_and(|n| n != album.items.len());
    album.completeness = Some(if (count_matches || (album.warnings.is_empty() && ended)) && !count_conflicts && !album.items.is_empty() {
        Completeness::Complete
    } else if album.expected_count.is_some_and(|n| n > album.items.len()) || !album.warnings.is_empty() || !post.media.is_empty() {
        Completeness::Partial
    } else { Completeness::Unknown });
    if album.completeness == Some(Completeness::Complete) { album.warnings.clear(); }
    else { album.warn(GALLERY_HELP); }
    album
}

pub(crate) async fn analyze(client: &Client, url: &str, session: &Headers) -> Result<ImageAlbum> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    analyze_with(url, session, &mut |url: String, headers: Headers| async move {
        tokio::time::timeout_at(deadline, fetch_page(client, &url, &headers)).await
            .unwrap_or_else(|_| Err("Facebook extraction reached its time limit. Available attachments were kept.".into()))
    }).await
}

// The fetch function keeps deterministic pagination/session tests off the network.
async fn analyze_with<F, Fut>(url: &str, session: &Headers, fetch: &mut F) -> Result<ImageAlbum>
where
    F: FnMut(String, Headers) -> Fut,
    Fut: Future<Output = std::result::Result<Page, String>>,
{
    let target = site::clean_facebook_url(url);
    let public = vec![("User-Agent".into(), sites::FB_CRAWLER_UA.into())];
    let mut album = match fetch(target.clone(), public.clone()).await {
        Ok(page) => extract(page, &public, fetch).await,
        Err(reason) => ImageAlbum { source_url: target.clone(), completeness: Some(Completeness::Unknown), warnings: vec![reason], ..Default::default() },
    };
    if album.completeness != Some(Completeness::Complete) && session.iter().any(|(k, v)| k.eq_ignore_ascii_case("cookie") && !v.is_empty()) {
        let mut browser = session.clone();
        if !browser.iter().any(|(k, _)| k.eq_ignore_ascii_case("user-agent")) { browser.push(("User-Agent".into(), BROWSER_UA.into())); }
        let mut crawler = browser.clone();
        crawler.retain(|(k, _)| !k.eq_ignore_ascii_case("user-agent"));
        crawler.push(("User-Agent".into(), sites::FB_CRAWLER_UA.into()));
        // Facebook sometimes refuses browser HTTP clients while still serving
        // authenticated crawler pages. These can carry the initial media set.
        for identity in [browser, crawler] {
        match fetch(target.clone(), identity.clone()).await {
            Ok(page) => {
                let authenticated = extract(page, &identity, fetch).await;
                merge_album(&mut album, authenticated);
            }
            Err(reason) => album.warn(reason),
        }
        if album.completeness == Some(Completeness::Complete) { break; }
        }
    }
    if album.items.is_empty() {
        return Err(Error::InvalidInput(album.warnings.join(" ")));
    }
    Ok(album)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const POST_URL: &str = "https://www.facebook.com/permalink.php?story_fbid=12345&id=98765";

    fn photo(id: usize) -> Value {
        json!({"__typename":"Photo", "id":id.to_string(),
            "image":{"uri":format!("https://cdn.example/{id}.jpg?signature=kept"),"width":100,"height":100},
            "url":format!("https://www.facebook.com/photo/?fbid={id}&set=pcb.12345")})
    }

    fn page(url: &str, data: Value) -> Page {
        Page { url: url.into(), html: format!("<title>Test post</title><script type=\"application/json\">{data}</script>") }
    }

    fn post_page(count: usize) -> Page {
        page(POST_URL, json!({"data":{"node":{"__typename":"Story", "post_id":"12345",
            "attachments":[{"all_subattachments":{"count":count,"nodes":(1..=5).map(photo).collect::<Vec<_>>()}}],
            "feedback":{"attachments":[{"media":photo(999)}]}}},
            "recommendations":[{"post_id":"22222","attachments":[{"media":photo(888)}]}]}))
    }

    fn slide_page(url: &str, id: usize, next: Option<usize>) -> Page {
        page(url, json!({"mediaSetToken":"pcb.12345", "media":photo(id),
            "nextMediaAfterNodeId":next.map(|n| json!({"__typename":"Photo","id":n.to_string()}))}))
    }

    #[tokio::test]
    async fn viewer_refresh_keeps_new_signatures_at_the_same_resolution() {
        let album = analyze_with(POST_URL, &vec![], &mut |url: String, _: Headers| {
            let parsed = Url::parse(&url).unwrap();
            let result = if let Some((_, id)) = parsed.query_pairs().find(|(k, _)| k == "fbid") {
                let id = id.parse::<usize>().unwrap();
                let mut media = photo(id); media["image"]["uri"] = json!(format!("https://cdn.example/{id}.jpg?signature=refreshed"));
                page(&url, json!({"mediaSetToken":"pcb.12345","media":media,"nextMediaAfterNodeId":(id < 7).then(|| json!({"id":(id+1).to_string()}))}))
            } else { post_page(7) };
            std::future::ready(Ok(result))
        }).await.unwrap();
        assert_eq!(album.items.len(), 7);
        assert!(album.items.iter().all(|i| i.url.ends_with("signature=refreshed")));
    }

    #[tokio::test]
    async fn missing_navigation_on_early_photos_recovers_from_the_last_preview() {
        let mut requested = Vec::new();
        let album = analyze_with(POST_URL, &vec![], &mut |url: String, _: Headers| {
            let parsed = Url::parse(&url).unwrap();
            let result = if let Some((_, id)) = parsed.query_pairs().find(|(k, _)| k == "fbid") {
                let id = id.parse::<usize>().unwrap(); requested.push(id);
                if id < 5 { page(&url, json!({"mediaSetToken":"pcb.12345", "media":photo(id)})) }
                else { slide_page(&url, id, (id < 7).then_some(id + 1)) }
            } else { post_page(7) };
            std::future::ready(Ok(result))
        }).await.unwrap();
        assert_eq!(requested, (1..=7).collect::<Vec<_>>());
        assert_eq!(album.items.iter().map(|i| i.id.clone()).collect::<Vec<_>>(), (1..=7).map(|i| format!("facebook:{i}")).collect::<Vec<_>>());
        assert_eq!(album.completeness, Some(Completeness::Complete));
        assert!(album.warnings.is_empty());
    }

    #[tokio::test]
    async fn viewer_connection_recovers_hidden_photos_without_a_next_link() {
        let album = analyze_with(POST_URL, &vec![], &mut |url: String, _: Headers| {
            std::future::ready(Ok(if url == POST_URL { post_page(7) } else {
                page(&url, json!({"mediaSetToken":"pcb.12345", "media":photo(1),
                    "photos":{"edges":(1..=7).map(|id| json!({"node":photo(id)})).collect::<Vec<_>>()}}))
            }))
        }).await.unwrap();
        assert_eq!(album.items.len(), 7);
        assert_eq!(album.completeness, Some(Completeness::Complete));
    }

    #[test]
    fn complementary_results_merge_and_conflicting_counts_stay_partial() {
        let item = |id| ImageItem::new(format!("facebook:{id}"), format!("https://scontent.fbcdn.net/{id}.jpg"));
        let mut first = ImageAlbum { items: (1..=5).map(item).collect(), expected_count: Some(7),
            completeness: Some(Completeness::Partial), ..Default::default() };
        let second = ImageAlbum { items: (4..=7).map(item).collect(), expected_count: Some(7), ..Default::default() };
        merge_album(&mut first, second);
        assert_eq!(first.items.len(), 7);
        assert_eq!(first.completeness, Some(Completeness::Complete));
        merge_album(&mut first, ImageAlbum { expected_count: Some(8), ..Default::default() });
        merge_album(&mut first, ImageAlbum { items: vec![item(8)], expected_count: Some(8), ..Default::default() });
        assert_eq!(first.completeness, Some(Completeness::Partial));
    }

    #[test]
    fn browser_metadata_is_post_scoped_and_cdn_only() {
        let mut good = photo(1);
        good["image"]["uri"] = json!("https://scontent.fbcdn.net/1.jpg?signature=kept");
        let doc = json!({"__typename":"Story", "post_id":"12345", "attachments":[{"all_subattachments":{"count":1,"nodes":[good]}}], "secret":"never cache"});
        let mut album = browser_album(POST_URL, &[doc.clone()]).unwrap();
        assert_eq!(album.completeness, Some(Completeness::Complete));
        browser_headers(&mut album, &vec![("User-Agent".into(), "Actual Browser".into()), ("Cookie".into(), "secret".into())]);
        assert!(album.items[0].headers.iter().any(|(k,v)| k == "User-Agent" && v == "Actual Browser"));
        assert!(!serde_json::to_string(&album).unwrap().contains("secret"));
        assert!(browser_album("https://facebook.com/posts/99999", &[doc.clone()]).is_err());
        assert!(browser_album(POST_URL, &[json!({"data":{"node_v2":{"__typename":"Story", "post_id":"99999", "attachments":doc["attachments"]}}})]).is_err());
        for bad in ["https://127.0.0.1/private.jpg", "https://fbcdn.net.evil.test/x.jpg", "https://fbcdn.net:123/x.jpg", "https://evil@fbcdn.net/x.jpg"] {
            let mut bad_doc = doc.clone(); bad_doc["attachments"][0]["all_subattachments"]["nodes"][0]["image"]["uri"] = json!(bad);
            assert!(browser_album(POST_URL, &[bad_doc]).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn missing_navigation_does_not_claim_a_sign_in_failure() {
        let album = analyze_with(POST_URL, &vec![], &mut |url: String, _: Headers| {
            std::future::ready(Ok(if url == POST_URL { post_page(7) } else { page(&url, json!({"mediaSetToken":"pcb.12345","media":photo(1)})) }))
        }).await.unwrap();
        assert_eq!(album.items.len(), 5);
        assert_eq!(album.completeness, Some(Completeness::Partial));
        assert!(!album.warnings.iter().any(|w| w == SIGN_IN));
        assert!(album.warnings.iter().any(|w| w == GALLERY_HELP));
    }

    #[tokio::test]
    async fn follows_five_previews_to_all_twenty_seven_in_order() {
        let mut requests = 0;
        let album = analyze_with(POST_URL, &vec![], &mut |url: String, _: Headers| {
            let parsed = Url::parse(&url).unwrap();
            let result = if let Some((_, id)) = parsed.query_pairs().find(|(k, _)| k == "fbid") {
                requests += 1;
                let id = id.parse::<usize>().unwrap();
                slide_page(&url, id, (id < 27).then_some(id + 1))
            } else { post_page(27) };
            std::future::ready(Ok(result))
        }).await.unwrap();
        assert_eq!(requests, 27);
        assert_eq!(album.items.len(), 27);
        assert_eq!(album.expected_count, Some(27));
        assert_eq!(album.completeness, Some(Completeness::Complete));
        assert!(album.warnings.is_empty());
        assert_eq!(album.items.iter().map(|i| i.id.clone()).collect::<Vec<_>>(),
            (1..=27).map(|i| format!("facebook:{i}")).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn session_retry_replaces_previews_without_leaking_credentials() {
        let mut requests = 0;
        let album = analyze_with(POST_URL, &vec![("Cookie".into(), "c_user=test; xs=secret".into())],
            &mut |_: String, headers: Headers| {
                requests += 1;
                let page = if requests == 1 {
                    assert!(!headers.iter().any(|(k, _)| k == "Cookie"));
                    Page { url: POST_URL.into(), html: "<meta property=\"og:image\" content=\"https://cdn.example/preview.jpg\">".into() }
                } else {
                    assert!(headers.iter().any(|(k, _)| k == "Cookie"));
                    assert!(headers.iter().any(|(k, v)| k == "User-Agent" && v == BROWSER_UA));
                    post_page(5)
                };
                std::future::ready(Ok(page))
            }).await.unwrap();
        assert_eq!(requests, 2);
        assert_eq!(album.items.len(), 5);
        assert_eq!(album.completeness, Some(Completeness::Complete));
        assert!(!serde_json::to_string(&album).unwrap().contains("secret"));
    }

    #[tokio::test]
    async fn denied_timed_out_rate_limited_and_looped_navigation_keep_previews() {
        for reason in [SIGN_IN, "Facebook rate-limited extraction.", "Facebook request timed out."] {
            let album = analyze_with(POST_URL, &vec![], &mut |url: String, _: Headers| {
                std::future::ready(if url == POST_URL { Ok(post_page(27)) } else { Err(reason.into()) })
            }).await.unwrap();
            assert_eq!(album.items.len(), 5);
            assert_eq!(album.completeness, Some(Completeness::Partial));
            assert!(album.warnings.iter().any(|w| w == reason));
        }
        let album = analyze_with(POST_URL, &vec![], &mut |url: String, _: Headers| {
            std::future::ready(Ok(if url == POST_URL { post_page(27) } else { slide_page(&url, 1, Some(1)) }))
        }).await.unwrap();
        assert_eq!(album.items.len(), 5);
        assert!(album.warnings.iter().any(|w| w.contains("repeated")));
        assert_eq!(album.completeness, Some(Completeness::Partial));
    }

    #[test]
    fn mixed_attachments_keep_videos_and_best_images_without_posters() {
        let video = json!({"__typename":"Video","id":"10001",
            "browser_native_hd_url":"https://video.fbcdn.net/clip.mp4?signature=kept",
            "image":{"uri":"https://cdn.example/poster.jpg"}});
        let mut image = photo(1);
        image["viewer_image"] = json!({"uri":"https://cdn.example/original.jpg?signature=original", "width":2000,"height":1000});
        let doc = json!({"post_id":"12345", "attachments":[{"all_subattachments":{"count":2,"nodes":[image,video]}}]});
        let post = parse_post(&[doc], &["12345".into()]);
        let items: Vec<_> = post.media.iter().filter_map(|n| media_item(n, &vec![])).collect();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].width, Some(2000));
        assert!(items[0].url.ends_with("signature=original"));
        assert_eq!(items[1].kind, super::super::MediaKind::Video);
        assert!(!items[1].formats.is_empty());
        assert_eq!(items[1].thumbnail.as_deref(), Some("https://cdn.example/poster.jpg"));
    }

    #[test]
    fn missing_conflicting_and_foreign_navigation_is_not_a_confirmed_end() {
        let unknown = parse_slide(&[json!({"mediaSetToken":"pcb.12345", "media":photo(1)})], "1", "pcb.12345");
        assert!(unknown.scoped);
        assert!(!unknown.terminal);
        assert!(unknown.next.is_none());
        let conflict = parse_slide(&[json!({"mediaSetToken":"pcb.12345", "nextMediaAfterNodeId":null,
            "other":{"nextMediaAfterNodeId":{"id":"2"}}})], "1", "pcb.12345");
        assert!(!conflict.terminal);
        assert!(conflict.next.is_none());
        let foreign = parse_slide(&[json!({"mediaSetToken":"a.999", "nextMediaAfterNodeId":null})], "1", "pcb.12345");
        assert!(!foreign.scoped);
    }

    #[test]
    fn facebook_ids_dedupe_renditions_without_collapsing_distinct_media() {
        let mut items = vec![ImageItem::new("facebook:1", "https://cdn.example/a.jpg"),
            ImageItem::new("facebook:1", "https://cdn.example/b.jpg").with_size(Some(2000), Some(1000)),
            ImageItem::new("facebook:2", "https://cdn.example/b.jpg")];
        super::super::dedupe(&mut items);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].width, Some(2000));
    }

    #[test]
    fn verification_and_wire_roundtrip_preserve_incomplete_results() {
        let old = r#"{"title":"Old", "sourceUrl":"https://facebook.com/posts/12345", "uploader":null,"items":[]}"#;
        let old: ImageAlbum = serde_json::from_str(old).unwrap();
        assert_eq!(old.completeness, None);
        assert!(old.warnings.is_empty());
        let mut album = ImageAlbum { completeness: Some(Completeness::Complete), expected_count: Some(2),
            items: vec![ImageItem::new("1", "https://cdn.example/1.jpg"), ImageItem::new("2", "https://cdn.example/2.jpg")], ..Default::default() };
        album.items[0].verified = Some(true);
        album.items[1].verified = Some(false);
        album.retain_verified();
        let encoded = serde_json::to_value(&album).unwrap();
        assert_eq!(encoded["completeness"], "partial");
        assert_eq!(encoded["expectedCount"], 2);
        let decoded: ImageAlbum = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded.items.len(), 1);
        assert!(!decoded.warnings.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires a Facebook URL and an existing extension-shared session vault"]
    async fn live_shared_session_post() {
        let url = std::env::var("FACEBOOK_TEST_URL").expect("FACEBOOK_TEST_URL");
        let dir = std::env::var("FACEBOOK_TEST_VAULT").expect("FACEBOOK_TEST_VAULT");
        let dir = std::path::Path::new(&dir);
        assert!(dir.join("sessions.vault").is_file(), "use an existing vault");
        let vault = crate::SessionVault::open(dir).unwrap();
        let session = vault.get("facebook.com").await.expect("share a Facebook session from the extension");
        let client = Client::builder().timeout(Duration::from_secs(30)).build().unwrap();
        let album = super::super::analyze(&client, &url, &session.to_headers()).await.unwrap();
        println!("found={}, expected={:?}, completeness={:?}, warnings={:?}", album.items.len(), album.expected_count, album.completeness, album.warnings);
        assert_eq!(album.completeness, Some(Completeness::Complete));
        assert!(!album.items.is_empty());
        if let Ok(directory) = std::env::var("FACEBOOK_TEST_DOWNLOAD_DIR") {
            use crate::downloader::{Downloader, Options, Outcome, Request, progress_channel};
            let downloader = Downloader::new(client, Options { connections: Some(1), ..Default::default() });
            let mut total = 0;
            for (index, item) in album.items.iter().enumerate() {
                assert_eq!(item.kind, super::super::MediaKind::Image, "this optional download check is for photo posts");
                let mut request = Request::new(&item.url, &directory);
                request.filename = Some(format!("facebook_{:02}.{}", index + 1, item.extension));
                request.headers = item.headers.clone();
                let (progress, _) = progress_channel(crate::model::DownloadId::new());
                let outcome = downloader.download(&request, tokio_util::sync::CancellationToken::new(), &progress).await;
                assert!(outcome.is_ok(), "download {} failed", index + 1);
                let Outcome::Completed { path, bytes } = outcome.unwrap() else { panic!("download paused"); };
                assert!(bytes > 0);
                let content = std::fs::read(path).unwrap();
                assert!(super::super::sniff::identify(&content).is_some(), "download must contain an image");
                total += bytes;
            }
            println!("downloaded {} images, {} bytes", album.items.len(), total);
        }
    }
}
