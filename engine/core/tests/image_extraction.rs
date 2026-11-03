//! Live checks for the image extractors.
//!
//! Every one of these talks to a real site, so they are `#[ignore]`d and never
//! run in an ordinary `cargo test`. They exist because a scraper's correctness
//! is not a property of its own code: it is a claim about somebody else's
//! payload, and the only thing that can falsify it is that payload. When a
//! platform changes shape, this is the file that says so.
//!
//! ```text
//! cargo test -p zoltraak-core --test image_extraction -- --ignored --nocapture
//! ```

use zoltraak_core::image;

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("client")
}

/// Print what an extractor found, so a failing run says *what* changed rather
/// than only that something did.
async fn report(url: &str) -> Option<image::ImageAlbum> {
    match image::analyze(&client(), url, &Vec::new()).await {
        Ok(album) => {
            println!("\n=== {url}");
            println!("    title:    {}", album.title);
            println!("    uploader: {:?}", album.uploader);
            for item in &album.items {
                println!(
                    "    [{}] {:?} {}x{} .{} verified={:?}",
                    item.id,
                    item.kind,
                    item.width.unwrap_or(0),
                    item.height.unwrap_or(0),
                    item.extension,
                    item.verified,
                );
                println!("         {}", item.url);
                // For a clip, the rendition list is the interesting part: it
                // is what the download will actually be run from.
                for format in &item.formats {
                    println!(
                        "         · {} {}x{} merge={} audio={:?}",
                        format.id,
                        format.width.unwrap_or(0),
                        format.height.unwrap_or(0),
                        format.requires_merge,
                        format.has_audio,
                    );
                }
            }
            Some(album)
        }
        Err(e) => {
            println!("\n=== {url}\n    FAILED: {e}");
            None
        }
    }
}

#[tokio::test]
#[ignore = "hits the network"]
async fn a_direct_image_link_resolves_to_itself_and_verifies() {
    let url = "https://upload.wikimedia.org/wikipedia/commons/3/3a/Cat03.jpg";
    let album = report(url).await.expect("direct image should resolve");

    assert_eq!(album.items.len(), 1);
    assert_eq!(album.items[0].url, url);
    // The whole point of the verification pass: the bytes agreed.
    assert_eq!(album.items[0].verified, Some(true));
    assert_eq!(album.items[0].extension, "jpg");
    assert!(album.items[0].filesize.is_some_and(|n| n > 0));
}

#[tokio::test]
#[ignore = "hits the network"]
async fn a_png_is_recognised_as_a_png_whatever_the_path_says() {
    // Served from a `.png` path — the extension and the magic bytes should
    // agree, which is the uninteresting case that must still hold.
    let url = "https://upload.wikimedia.org/wikipedia/commons/4/47/PNG_transparency_demonstration_1.png";
    let album = report(url).await.expect("direct png should resolve");

    assert_eq!(album.items[0].extension, "png");
    assert_eq!(album.items[0].verified, Some(true));
}

#[tokio::test]
#[ignore = "hits the network"]
async fn an_ordinary_page_falls_back_to_its_open_graph_image() {
    // No platform extractor claims this, so it exercises the generic path.
    let album = report("https://en.wikipedia.org/wiki/Cat").await;
    let album = album.expect("a page with an og:image should resolve");

    assert!(!album.items.is_empty());
    assert!(album.items.iter().all(|i| i.verified != Some(false)));
}

#[tokio::test]
#[ignore = "hits the network"]
async fn a_page_with_no_pictures_is_an_error_not_an_empty_album() {
    // An empty album reaching the UI would render as a grid of nothing with no
    // explanation; the caller is told instead.
    let result = image::analyze(&client(), "https://example.com/", &Vec::new()).await;
    println!("\n=== example.com\n    {result:?}");
    assert!(result.is_err());
}

#[tokio::test]
#[ignore = "hits the network; X rate-limits aggressively"]
async fn an_x_photo_post_yields_its_originals() {
    // Reported, not asserted beyond the shape: the syndication endpoint rate-
    // limits by IP, so a failure here is as likely to be the limiter as a
    // regression. Read the printout.
    if let Some(album) = report("https://x.com/NASA/status/1750321437123456789").await {
        for item in &album.items {
            assert!(item.url.contains("name=orig"), "originals, not thumbnails");
        }
    }
}

#[tokio::test]
#[ignore = "hits the network"]
async fn a_reddit_gallery_yields_every_slide_in_order() {
    // Reported rather than asserted: Reddit answers datacenter IPs with a 403
    // often enough that a hard assertion would fail for the wrong reason.
    report("https://www.reddit.com/r/pics/comments/1b1o3ks/").await;
}

#[tokio::test]
#[ignore = "hits the network"]
async fn a_reddit_video_post_yields_a_clip_and_not_its_poster_frame() {
    // Two things worth seeing in the printout: exactly one item (the preview
    // image is the clip's poster, not separate content), and a format that can
    // actually carry sound — Reddit's `fallback_url` is video-only, so a
    // manifest has to have been resolved.
    if let Some(album) = report("https://www.reddit.com/r/aww/comments/1cx8t2p/").await {
        if let Some(clip) = album.items.iter().find(|i| i.kind == image::MediaKind::Video) {
            assert!(!clip.formats.is_empty(), "a clip must carry its renditions");
        }
    }
}

/// Every clip the post path emits must be downloadable by the video pipeline —
/// which means a first format with a URL. An item that fails this would render
/// a tile whose download button cannot work.
#[tokio::test]
#[ignore = "hits the network"]
async fn every_clip_carries_a_fetchable_rendition() {
    for url in [
        "https://www.reddit.com/r/aww/comments/1cx8t2p/",
        "https://en.wikipedia.org/wiki/Cat",
    ] {
        let Some(album) = report(url).await else { continue };
        for item in album.items.iter().filter(|i| i.kind == image::MediaKind::Video) {
            let best = item.formats.first().expect("a clip always has a format");
            assert!(best.video_url.is_some(), "{}: clip {} has no source", url, item.id);
        }
        // And the converse: a picture never carries formats, because that is
        // the flag the UI routes on.
        for item in album.items.iter().filter(|i| i.kind == image::MediaKind::Image) {
            assert!(item.formats.is_empty(), "{}: image {} carried formats", url, item.id);
        }
    }
}
