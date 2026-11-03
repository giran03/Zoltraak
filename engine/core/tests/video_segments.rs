//! The concurrent segment fetcher.
//!
//! A real origin, not a mocked client: what matters here is how the fetcher
//! behaves against actual concurrent HTTP — ordering when responses land out
//! of order, how many requests it really has in flight, and whether one flaky
//! segment takes the whole stream down with it.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use zoltraak_core::video::stream::{
    Segment, SegmentOptions, StreamOptions, StreamOutcome, StreamProgress, StreamTick,
    download_concat_with_options, download_ranged_with_options, download_single_with_progress,
};
use zoltraak_core::video::youtube::ChunkPlan;
use tokio_util::sync::CancellationToken;

const SEGMENT_LEN: usize = 4096;
const SEGMENT_COUNT: usize = 12;

/// Deterministic, segment-dependent bytes, so a mis-ordered or duplicated
/// segment shows up as a content mismatch rather than passing by luck.
fn segment_payload(index: usize) -> Vec<u8> {
    (0..SEGMENT_LEN).map(|i| ((index * 31 + i) % 251) as u8).collect()
}

fn expected_concatenation(count: usize) -> Vec<u8> {
    (0..count).flat_map(segment_payload).collect()
}

struct Origin {
    /// Requests currently being served, and the high-water mark across the run.
    in_flight: AtomicUsize,
    peak_in_flight: AtomicUsize,
    requests: AtomicUsize,
    last_range: AtomicUsize,
    /// Segment index that fails, and how many times it fails before serving.
    flaky_segment: Option<usize>,
    flaky_remaining: AtomicU64,
    /// Status returned by the flaky segment while it is failing.
    flaky_status: StatusCode,
    /// Later segments are served *faster*, so responses land in the opposite
    /// order to the playlist. A fetcher that wrote in completion order rather
    /// than playlist order would produce a scrambled file.
    inverted_delay: bool,
    /// Ignore `Range` and always return the whole object, to exercise the
    /// client-side slicing path.
    ignore_range: bool,
    /// Serve at most this many bytes per distinct URL (query string included)
    /// of `/file.mp4`, then 403 — the shape of a CDN link with a bounded
    /// allowance.
    allowance_per_link: Option<usize>,
    served_per_link: std::sync::Mutex<std::collections::HashMap<String, usize>>,
}

impl Origin {
    fn new() -> Self {
        Self {
            in_flight: AtomicUsize::new(0),
            peak_in_flight: AtomicUsize::new(0),
            requests: AtomicUsize::new(0),
            last_range: AtomicUsize::new(usize::MAX),
            flaky_segment: None,
            flaky_remaining: AtomicU64::new(0),
            flaky_status: StatusCode::SERVICE_UNAVAILABLE,
            inverted_delay: false,
            ignore_range: false,
            allowance_per_link: None,
            served_per_link: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

async fn serve_segment(
    State(origin): State<Arc<Origin>>,
    AxumPath(index): AxumPath<usize>,
    headers: HeaderMap,
) -> Response {
    origin.requests.fetch_add(1, Ordering::SeqCst);
    let now = origin.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
    origin.peak_in_flight.fetch_max(now, Ordering::SeqCst);

    // Guard against an early return leaving the in-flight count wrong.
    struct Leave(Arc<Origin>);
    impl Drop for Leave {
        fn drop(&mut self) {
            self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let _leave = Leave(Arc::clone(&origin));

    if origin.flaky_segment == Some(index)
        && origin.flaky_remaining.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok()
    {
        return (origin.flaky_status, "temporarily unavailable").into_response();
    }

    let delay = if origin.inverted_delay {
        // Segment 0 waits longest; the last segment returns immediately.
        Duration::from_millis((SEGMENT_COUNT - index) as u64 * 12)
    } else {
        Duration::from_millis(20)
    };
    tokio::time::sleep(delay).await;

    let body = segment_payload(index);

    match headers.get(axum::http::header::RANGE) {
        Some(value) if !origin.ignore_range => {
            // `bytes=start-last`, inclusive on both ends.
            let raw = value.to_str().unwrap_or_default().trim_start_matches("bytes=").to_string();
            let (start, last) = raw.split_once('-').expect("well-formed range");
            let start: usize = start.parse().expect("range start");
            let last: usize = last.parse().expect("range end");
            let slice = body[start..=last.min(body.len() - 1)].to_vec();
            (StatusCode::PARTIAL_CONTENT, Body::from(slice)).into_response()
        }
        // Either no range was asked for, or this origin ignores ranges and
        // answers 200 with the whole object.
        _ => (StatusCode::OK, Body::from(body)).into_response(),
    }
}

/// One whole object, honouring `Range` — the single-file (`.mp4`) source.
/// Both open-ended (`bytes=N-`) and bounded (`bytes=N-M`) ranges are served,
/// the latter being what a ranged chunk plan asks for.
async fn serve_file(
    State(origin): State<Arc<Origin>>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
    headers: HeaderMap,
) -> Response {
    origin.requests.fetch_add(1, Ordering::SeqCst);
    let now = origin.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
    origin.peak_in_flight.fetch_max(now, Ordering::SeqCst);
    let body = expected_concatenation(SEGMENT_COUNT);

    let mut response = match headers.get(axum::http::header::RANGE) {
        Some(value) if !origin.ignore_range => {
            let raw = value.to_str().unwrap_or_default().trim_start_matches("bytes=").to_string();
            let (start, end) = raw.split_once('-').expect("a range");
            let start: usize = start.parse().expect("range start");
            origin.last_range.store(start, Ordering::SeqCst);
            let end: usize = end.parse::<usize>().map(|e| (e + 1).min(body.len())).unwrap_or(body.len());
            let refused = match origin.allowance_per_link {
                Some(allowance) => {
                    let mut served = origin.served_per_link.lock().unwrap();
                    let used = served.entry(query.clone().unwrap_or_default()).or_insert(0);
                    if *used + (end - start) > allowance {
                        true
                    } else {
                        *used += end - start;
                        false
                    }
                }
                None => false,
            };
            if refused {
                (StatusCode::FORBIDDEN, Body::from("allowance exhausted")).into_response()
            } else {
                (StatusCode::PARTIAL_CONTENT,
                    [(axum::http::header::CONTENT_RANGE, format!("bytes {start}-{}/{}", end - 1, body.len()))],
                    Body::from(body[start..end].to_vec())).into_response()
            }
        }
        _ => (StatusCode::OK, Body::from(body)).into_response(),
    };
    response.headers_mut().insert(axum::http::header::ETAG, "\"fixture-v1\"".parse().unwrap());
    // Hold the slot briefly so concurrency is observable.
    tokio::time::sleep(Duration::from_millis(30)).await;
    origin.in_flight.fetch_sub(1, Ordering::SeqCst);
    response
}

async fn spawn_origin(origin: Arc<Origin>) -> SocketAddr {
    let app = Router::new()
        .route("/seg/{index}", get(serve_segment))
        .route("/file.mp4", get(serve_file))
        .with_state(origin);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

fn segments_for(addr: SocketAddr, count: usize) -> Vec<Segment> {
    (0..count)
        .map(|i| Segment::whole(format!("http://{addr}/seg/{i}")))
        .collect()
}

#[tokio::test]
async fn segments_are_written_in_playlist_order_even_when_they_arrive_backwards() {
    let mut origin = Origin::new();
    origin.inverted_delay = true;
    let origin = Arc::new(origin);
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("out.ts");

    let options = StreamOptions {
        segments: SegmentOptions {
            concurrency: 6,
            ..Default::default()
        },
        limiter: None,
        ..Default::default()
    };

    let written = download_concat_with_options(
        &reqwest::Client::new(),
        &segments_for(addr, SEGMENT_COUNT),
        &Vec::new(),
        &dest,
        None,
        &options,
    )
    .await
    .expect("download succeeds");

    let actual = std::fs::read(&dest).expect("read output");
    assert_eq!(written.bytes() as usize, SEGMENT_LEN * SEGMENT_COUNT);
    assert_eq!(
        actual,
        expected_concatenation(SEGMENT_COUNT),
        "segments must be concatenated in playlist order, not completion order"
    );
}

#[tokio::test]
async fn segments_are_actually_fetched_concurrently() {
    let origin = Arc::new(Origin::new());
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("out.ts");

    let options = StreamOptions {
        segments: SegmentOptions {
            concurrency: 4,
            ..Default::default()
        },
        limiter: None,
        ..Default::default()
    };

    download_concat_with_options(
        &reqwest::Client::new(),
        &segments_for(addr, SEGMENT_COUNT),
        &Vec::new(),
        &dest,
        None,
        &options,
    )
    .await
    .expect("download succeeds");

    let peak = origin.peak_in_flight.load(Ordering::SeqCst);
    assert!(
        peak > 1,
        "the whole point is overlapping requests; peak in flight was {peak}"
    );
    assert!(
        peak <= 4,
        "concurrency must stay bounded by the option; peak in flight was {peak}"
    );
}

#[tokio::test]
async fn concurrency_of_one_still_produces_the_same_file() {
    let origin = Arc::new(Origin::new());
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("out.ts");

    let options = StreamOptions {
        segments: SegmentOptions {
            concurrency: 1,
            ..Default::default()
        },
        limiter: None,
        ..Default::default()
    };

    download_concat_with_options(
        &reqwest::Client::new(),
        &segments_for(addr, SEGMENT_COUNT),
        &Vec::new(),
        &dest,
        None,
        &options,
    )
    .await
    .expect("download succeeds");

    assert_eq!(std::fs::read(&dest).expect("read"), expected_concatenation(SEGMENT_COUNT));
    assert_eq!(
        origin.peak_in_flight.load(Ordering::SeqCst),
        1,
        "concurrency 1 must serialize, so the sequential path stays exercised"
    );
}

#[tokio::test]
async fn one_flaky_segment_is_retried_rather_than_failing_the_stream() {
    let mut origin = Origin::new();
    origin.flaky_segment = Some(5);
    origin.flaky_remaining = AtomicU64::new(2);
    let origin = Arc::new(origin);
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("out.ts");

    let options = StreamOptions {
        segments: SegmentOptions {
            concurrency: 4,
            retry: zoltraak_core::retry::RetryPolicy {
                max_attempts: 3,
                base: Duration::from_millis(10),
                max_delay: Duration::from_millis(50),
            },
        },
        limiter: None,
        ..Default::default()
    };

    download_concat_with_options(
        &reqwest::Client::new(),
        &segments_for(addr, SEGMENT_COUNT),
        &Vec::new(),
        &dest,
        None,
        &options,
    )
    .await
    .expect("two transient failures on one segment must not fail the stream");

    assert_eq!(
        std::fs::read(&dest).expect("read"),
        expected_concatenation(SEGMENT_COUNT),
        "the retried segment must land in its original position"
    );
    assert_eq!(
        origin.requests.load(Ordering::SeqCst),
        SEGMENT_COUNT + 2,
        "exactly the two failed attempts should be repeated"
    );
}

#[tokio::test]
async fn a_permanent_segment_failure_still_fails_the_stream() {
    let mut origin = Origin::new();
    origin.flaky_segment = Some(3);
    origin.flaky_remaining = AtomicU64::new(u64::MAX);
    origin.flaky_status = StatusCode::NOT_FOUND;
    let origin = Arc::new(origin);
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("out.ts");

    let error = download_concat_with_options(
        &reqwest::Client::new(),
        &segments_for(addr, SEGMENT_COUNT),
        &Vec::new(),
        &dest,
        None,
        &StreamOptions::default(),
    )
    .await
    .expect_err("a 404 is not retryable and must surface");

    match error {
        zoltraak_core::Error::UnexpectedStatus { status, .. } => assert_eq!(status, 404),
        other => panic!("expected an UnexpectedStatus, got {other:?}"),
    }

    // A 404 is terminal, so the segment is attempted once and not retried.
    assert!(
        origin.requests.load(Ordering::SeqCst) <= SEGMENT_COUNT,
        "a non-retryable status must not be retried"
    );
}

#[tokio::test]
async fn byte_range_segments_are_sliced_when_the_origin_ignores_range() {
    let mut origin = Origin::new();
    origin.ignore_range = true;
    let origin = Arc::new(origin);
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("out.mp4");

    // Three slices of the same object, the shape fMP4 HLS uses for its init
    // segment and every media segment after it.
    let url = format!("http://{addr}/seg/0");
    let segments = vec![
        Segment { url: url.clone(), range: Some((0, 100)) },
        Segment { url: url.clone(), range: Some((100, 250)) },
        Segment { url: url.clone(), range: Some((350, 50)) },
    ];

    let written = download_concat_with_options(
        &reqwest::Client::new(),
        &segments,
        &Vec::new(),
        &dest,
        None,
        &StreamOptions::default(),
    )
    .await
    .expect("download succeeds");

    let full = segment_payload(0);
    let mut expected = Vec::new();
    expected.extend_from_slice(&full[0..100]);
    expected.extend_from_slice(&full[100..350]);
    expected.extend_from_slice(&full[350..400]);

    assert_eq!(written.bytes(), 400);
    assert_eq!(std::fs::read(&dest).expect("read"), expected);
}

/// Not a correctness test, and deliberately not part of the default run —
/// wall-clock assertions are flaky on a loaded machine. Run it by hand
/// (`cargo test -p zoltraak-core --test video_segments -- --ignored --nocapture`)
/// to see the latency-bound gap the concurrency exists to close.
#[tokio::test]
#[ignore = "timing-dependent; run manually"]
async fn measure_sequential_against_concurrent() {
    async fn run(concurrency: usize, addr: SocketAddr, dir: &std::path::Path) -> Duration {
        let dest = dir.join(format!("out-{concurrency}.ts"));
        let options = StreamOptions {
            segments: SegmentOptions {
                concurrency,
                ..Default::default()
            },
            limiter: None,
            ..Default::default()
        };
        let started = std::time::Instant::now();
        download_concat_with_options(
            &reqwest::Client::new(),
            &segments_for(addr, SEGMENT_COUNT),
            &Vec::new(),
            &dest,
            None,
            &options,
        )
        .await
        .expect("download succeeds");
        started.elapsed()
    }

    let origin = Arc::new(Origin::new());
    let addr = spawn_origin(Arc::clone(&origin)).await;
    let dir = tempfile::tempdir().expect("tempdir");

    let sequential = run(1, addr, dir.path()).await;
    let concurrent = run(6, addr, dir.path()).await;

    println!(
        "{SEGMENT_COUNT} segments, 20ms origin latency each:\n  \
         concurrency 1: {sequential:?}\n  \
         concurrency 6: {concurrent:?}\n  \
         speedup: {:.2}x",
        sequential.as_secs_f64() / concurrent.as_secs_f64()
    );
}

#[tokio::test]
async fn concurrent_segments_share_one_bandwidth_cap_rather_than_multiplying_it() {
    let origin = Arc::new(Origin::new());
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("out.ts");

    // 49,152 bytes at 16 KiB/s. The bucket starts full, so one second's worth
    // is free and the remaining 32,768 bytes take ~2s. If each in-flight
    // segment were throttled independently, six of them would sleep in
    // parallel and the whole thing would finish in a fraction of that — which
    // is exactly the bug the shared lock exists to prevent.
    const CAP: u64 = 16 * 1024;
    let limiter: zoltraak_core::SharedRateLimit = Arc::new(tokio::sync::Mutex::new(
        zoltraak_core::speed::RateLimit::new(Some(CAP), std::time::Instant::now()),
    ));

    let options = StreamOptions {
        segments: SegmentOptions {
            concurrency: 6,
            ..Default::default()
        },
        limiter: Some(Arc::clone(&limiter)),
        ..Default::default()
    };

    let started = std::time::Instant::now();
    download_concat_with_options(
        &reqwest::Client::new(),
        &segments_for(addr, SEGMENT_COUNT),
        &Vec::new(),
        &dest,
        None,
        &options,
    )
    .await
    .expect("download succeeds");
    let elapsed = started.elapsed();

    assert_eq!(std::fs::read(&dest).expect("read"), expected_concatenation(SEGMENT_COUNT));
    assert!(
        elapsed >= Duration::from_millis(1200),
        "six concurrent segments must add up to one {CAP} B/s cap, not six of them; \
         transferred {} bytes in {elapsed:?}",
        SEGMENT_LEN * SEGMENT_COUNT
    );
}

#[tokio::test]
async fn progress_is_monotonic_and_ends_at_the_full_size() {
    let origin = Arc::new(Origin::new());
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("out.ts");

    let highest = Arc::new(AtomicU64::new(0));
    let regressions = Arc::new(AtomicUsize::new(0));
    let last_total = Arc::new(AtomicU64::new(0));

    {
        let highest = Arc::clone(&highest);
        let regressions = Arc::clone(&regressions);
        let last_total = Arc::clone(&last_total);
        let on_progress = move |tick: StreamTick| {
            let previous = highest.fetch_max(tick.bytes, Ordering::SeqCst);
            if tick.bytes < previous {
                regressions.fetch_add(1, Ordering::SeqCst);
            }
            if let Some(total) = tick.total {
                last_total.store(total, Ordering::SeqCst);
            }
        };

        download_concat_with_options(
            &reqwest::Client::new(),
            &segments_for(addr, SEGMENT_COUNT),
            &Vec::new(),
            &dest,
            Some(&on_progress),
            &StreamOptions::default(),
        )
        .await
        .expect("download succeeds");
    }

    assert_eq!(
        regressions.load(Ordering::SeqCst),
        0,
        "a progress bar that goes backwards is worse than none"
    );
    assert_eq!(
        highest.load(Ordering::SeqCst) as usize,
        SEGMENT_LEN * SEGMENT_COUNT,
        "progress must finish at the real total, not short of it"
    );

    // With equal-sized segments the extrapolation should land exactly on the
    // real total, so a segmented download gets a determinate progress bar
    // instead of an indefinite spinner.
    assert_eq!(
        last_total.load(Ordering::SeqCst) as usize,
        SEGMENT_LEN * SEGMENT_COUNT,
        "the size estimate must converge on the real total"
    );
}


// ---------------------------------------------------------------------------
// Cancellation and resume
// ---------------------------------------------------------------------------

/// Run a segmented download that gets cancelled partway, and hand back where it
/// stopped. The origin is slow enough per segment that the cancel lands in the
/// middle rather than after everything has already been fetched.
async fn download_then_cancel(
    addr: SocketAddr,
    dest: &std::path::Path,
    _cancel_after: Duration,
) -> StreamProgress {
    let cancel = CancellationToken::new();
    let options = StreamOptions {
        segments: SegmentOptions {
            concurrency: 2,
            ..Default::default()
        },
        limiter: None,
        resume: StreamProgress::default(),
        cancel: cancel.clone(),
        live_seconds: None,
    };

    let segments = segments_for(addr, SEGMENT_COUNT);
    let dest_owned = dest.to_path_buf();
    let task = tokio::spawn(async move {
        download_concat_with_options(
            &reqwest::Client::new(),
            &segments,
            &Vec::new(),
            &dest_owned,
            None,
            &options,
        )
        .await
    });

    let start = std::time::Instant::now();
    loop {
        let on_disk = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        if on_disk >= SEGMENT_LEN as u64 || start.elapsed() > Duration::from_millis(500) {
            cancel.cancel();
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    match task.await.expect("task joins").expect("cancelling is not an error") {
        StreamOutcome::Paused(progress) => progress,
        StreamOutcome::Completed { .. } => panic!("the download finished before it could be cancelled"),
        StreamOutcome::Refused { .. } => panic!("nothing refused this download"),
    }
}

#[tokio::test]
async fn cancelling_stops_at_a_segment_boundary_and_reports_what_is_on_disk() {
    let origin = Arc::new(Origin::new());
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("out.ts");

    let progress = download_then_cancel(addr, &dest, Duration::from_millis(60)).await;

    assert!(progress.segments_done > 0, "some progress should have been kept");
    assert!(
        progress.segments_done < SEGMENT_COUNT,
        "the run must have been cut short, not completed"
    );
    assert_eq!(
        progress.bytes as usize,
        progress.segments_done * SEGMENT_LEN,
        "a pause must land on a whole-segment boundary, never mid-segment"
    );

    let on_disk = std::fs::metadata(&dest).expect("partial file exists").len();
    assert_eq!(
        on_disk, progress.bytes,
        "the reported position must match the file, or a resume writes into a hole"
    );
    assert!(progress.fingerprint.is_some(), "a resumable pause must record what it was fetching");
}

#[tokio::test]
async fn a_resumed_download_produces_the_same_file_as_an_uninterrupted_one() {
    let origin = Arc::new(Origin::new());
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("out.ts");

    let progress = download_then_cancel(addr, &dest, Duration::from_millis(60)).await;
    let resumed_from = progress.segments_done;

    let options = StreamOptions {
        segments: SegmentOptions {
            concurrency: 4,
            ..Default::default()
        },
        limiter: None,
        resume: progress,
        cancel: CancellationToken::new(),
        live_seconds: None,
    };

    let outcome = download_concat_with_options(
        &reqwest::Client::new(),
        &segments_for(addr, SEGMENT_COUNT),
        &Vec::new(),
        &dest,
        None,
        &options,
    )
    .await
    .expect("resume succeeds");

    assert_eq!(outcome.bytes() as usize, SEGMENT_LEN * SEGMENT_COUNT);
    assert_eq!(
        std::fs::read(&dest).expect("read"),
        expected_concatenation(SEGMENT_COUNT),
        "a resumed file must be byte-identical to one downloaded in a single run"
    );
    assert!(
        resumed_from > 0,
        "the test is only meaningful if the resume actually skipped work"
    );
}

#[tokio::test]
async fn a_changed_playlist_restarts_instead_of_splicing_two_streams() {
    let origin = Arc::new(Origin::new());
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("out.ts");

    let mut progress = download_then_cancel(addr, &dest, Duration::from_millis(60)).await;

    // The playlist this progress was measured against is gone: a live stream
    // rolled its window, so segment index N now names different bytes.
    progress.fingerprint = Some("some-other-playlist".to_string());

    let options = StreamOptions {
        resume: progress,
        cancel: CancellationToken::new(),
        ..Default::default()
    };

    let outcome = download_concat_with_options(
        &reqwest::Client::new(),
        &segments_for(addr, SEGMENT_COUNT),
        &Vec::new(),
        &dest,
        None,
        &options,
    )
    .await
    .expect("mismatched resume state restarts rather than failing");

    assert_eq!(outcome.bytes() as usize, SEGMENT_LEN * SEGMENT_COUNT);
    assert_eq!(
        std::fs::read(&dest).expect("read"),
        expected_concatenation(SEGMENT_COUNT),
        "restarting must produce a clean file, not one with a spliced prefix"
    );
}

#[tokio::test]
async fn resume_restarts_when_the_partial_file_is_gone() {
    let origin = Arc::new(Origin::new());
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("out.ts");

    let progress = download_then_cancel(addr, &dest, Duration::from_millis(60)).await;
    std::fs::remove_file(&dest).expect("delete the partial file");

    let options = StreamOptions {
        resume: progress,
        cancel: CancellationToken::new(),
        ..Default::default()
    };

    let outcome = download_concat_with_options(
        &reqwest::Client::new(),
        &segments_for(addr, SEGMENT_COUNT),
        &Vec::new(),
        &dest,
        None,
        &options,
    )
    .await
    .expect("a missing partial file restarts rather than failing");

    assert_eq!(
        outcome.bytes() as usize,
        SEGMENT_LEN * SEGMENT_COUNT,
        "the restart must account for the whole stream, not just the tail it thought was left"
    );
    assert_eq!(
        std::fs::read(&dest).expect("read"),
        expected_concatenation(SEGMENT_COUNT)
    );
}

#[tokio::test]
async fn resume_trims_a_partial_segment_written_past_the_boundary() {
    let origin = Arc::new(Origin::new());
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("out.ts");

    let progress = download_then_cancel(addr, &dest, Duration::from_millis(60)).await;

    // Simulate a crash that left a torn segment past the recorded boundary.
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().append(true).open(&dest).expect("open");
        file.write_all(b"torn-tail-that-must-not-survive").expect("append junk");
    }

    let options = StreamOptions {
        resume: progress,
        cancel: CancellationToken::new(),
        ..Default::default()
    };

    download_concat_with_options(
        &reqwest::Client::new(),
        &segments_for(addr, SEGMENT_COUNT),
        &Vec::new(),
        &dest,
        None,
        &options,
    )
    .await
    .expect("resume succeeds");

    assert_eq!(
        std::fs::read(&dest).expect("read"),
        expected_concatenation(SEGMENT_COUNT),
        "the torn tail must be truncated, not left embedded in the stream"
    );
}

#[tokio::test]
async fn a_single_file_resumes_with_an_http_range() {
    let origin = Arc::new(Origin::new());
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("clip.mp4");

    let whole = expected_concatenation(SEGMENT_COUNT);
    let prefix = 5000u64;
    std::fs::write(&dest, &whole[..prefix as usize]).expect("seed a partial file");

    let options = StreamOptions {
        resume: StreamProgress {
            segments_done: 0,
            bytes: prefix,
            fingerprint: Some(format!("file-v2:{}", serde_json::json!({
                "source": format!("http://{addr}/file.mp4"), "etag": "\"fixture-v1\"", "total": whole.len()
            }))),
        },
        cancel: CancellationToken::new(),
        ..Default::default()
    };

    let outcome = download_single_with_progress(
        &reqwest::Client::new(),
        &format!("http://{addr}/file.mp4"),
        &Vec::new(),
        &dest,
        None,
        &options,
    )
    .await
    .expect("range resume succeeds");

    assert_eq!(outcome.bytes(), whole.len() as u64);
    assert_eq!(std::fs::read(&dest).expect("read"), whole);
    assert_eq!(origin.last_range.load(Ordering::SeqCst), prefix as usize);
}

#[tokio::test]
async fn a_changed_validator_never_appends_to_a_partial_file() {
    let origin = Arc::new(Origin::new());
    let addr = spawn_origin(Arc::clone(&origin)).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("clip.mp4");
    let prefix = vec![0xAA; 5000];
    std::fs::write(&dest, &prefix).unwrap();
    let options = StreamOptions {
        resume: StreamProgress { bytes: 5000, fingerprint: Some(format!("file-v2:{}", serde_json::json!({
            "source": format!("http://{addr}/file.mp4"), "etag": "\"old-version\"", "total": SEGMENT_COUNT * SEGMENT_LEN
        }))), ..Default::default() }, ..Default::default()
    };
    let error = download_single_with_progress(&reqwest::Client::new(), &format!("http://{addr}/file.mp4"),
        &Vec::new(), &dest, None, &options).await.unwrap_err();
    assert!(matches!(error, zoltraak_core::Error::StaleResume));
    assert_eq!(std::fs::read(&dest).unwrap(), prefix);
}

#[tokio::test]
async fn legacy_progress_without_a_validator_restarts_safely() {
    let origin = Arc::new(Origin::new());
    let addr = spawn_origin(Arc::clone(&origin)).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("clip.mp4");
    std::fs::write(&dest, vec![0xAA; 5000]).unwrap();
    let options = StreamOptions { resume: StreamProgress { bytes: 5000, ..Default::default() }, ..Default::default() };
    download_single_with_progress(&reqwest::Client::new(), &format!("http://{addr}/file.mp4"),
        &Vec::new(), &dest, None, &options).await.unwrap();
    assert_eq!(origin.last_range.load(Ordering::SeqCst), usize::MAX);
    assert_eq!(std::fs::read(&dest).unwrap(), expected_concatenation(SEGMENT_COUNT));
}

#[tokio::test]
async fn a_single_file_restarts_when_the_origin_ignores_range() {
    let mut origin = Origin::new();
    origin.ignore_range = true;
    let origin = Arc::new(origin);
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("clip.mp4");

    let whole = expected_concatenation(SEGMENT_COUNT);
    // Deliberately wrong bytes: if the fetcher appended to them instead of
    // replacing them, the result would be longer than the real file.
    std::fs::write(&dest, vec![0xAAu8; 5000]).expect("seed a bogus partial");

    let options = StreamOptions {
        resume: StreamProgress {
            segments_done: 0,
            bytes: 5000,
            fingerprint: None,
        },
        cancel: CancellationToken::new(),
        ..Default::default()
    };

    let outcome = download_single_with_progress(
        &reqwest::Client::new(),
        &format!("http://{addr}/file.mp4"),
        &Vec::new(),
        &dest,
        None,
        &options,
    )
    .await
    .expect("a 200 response restarts the file");

    assert_eq!(outcome.bytes(), whole.len() as u64);
    assert_eq!(
        std::fs::read(&dest).expect("read"),
        whole,
        "a server that ignored the range must overwrite, never append"
    );
}

/// A stream fetched as a chunk plan (the googlevideo path) is byte-identical
/// to the file, is fetched with several ranges in flight, and — the point of
/// the plan's own identity — resumes after the URL itself has changed, as it
/// does when an expired link is re-resolved from a different CDN edge.
#[tokio::test]
async fn a_ranged_plan_downloads_in_parallel_and_resumes_across_a_url_change() {
    let origin = Arc::new(Origin::new());
    let addr = spawn_origin(Arc::clone(&origin)).await;
    let total = (SEGMENT_LEN * SEGMENT_COUNT) as u64;
    let plan = ChunkPlan { total, chunk_size: SEGMENT_LEN as u64, identity: "yt:o-abc:137:1:49152".into() };

    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("ranged.mp4");
    let cancel = CancellationToken::new();
    let options = StreamOptions { cancel: cancel.clone(), ..StreamOptions::default() };

    // Cancel part-way through: the outcome names how many whole chunks are on
    // disk, against the plan's identity rather than the URL.
    let cancel_soon = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(45)).await;
        cancel_soon.cancel();
    });
    let first = download_ranged_with_options(
        &reqwest::Client::new(),
        &format!("http://{addr}/file.mp4?expire=1&sig=old"),
        &plan,
        &Vec::new(),
        &dest,
        None,
        &options,
    )
    .await
    .expect("first run");
    let progress = match first {
        StreamOutcome::Paused(progress) => progress,
        StreamOutcome::Refused { .. } => panic!("nothing refused this download"),
        StreamOutcome::Completed { .. } => {
            // The origin was fast enough to finish; still a valid file.
            assert_eq!(std::fs::read(&dest).unwrap(), expected_concatenation(SEGMENT_COUNT));
            return;
        }
    };
    assert!(progress.fingerprint.as_deref().unwrap().contains("yt:o-abc:137:1:49152"), "{progress:?}");

    // Resume from a *different* URL for the same bytes.
    let options = StreamOptions { resume: progress.clone(), ..StreamOptions::default() };
    let requests_before = origin.requests.load(Ordering::SeqCst);
    let second = download_ranged_with_options(
        &reqwest::Client::new(),
        &format!("http://{addr}/file.mp4?expire=2&sig=new"),
        &plan,
        &Vec::new(),
        &dest,
        None,
        &options,
    )
    .await
    .expect("second run");
    assert_eq!(second.bytes(), total);
    assert_eq!(std::fs::read(&dest).unwrap(), expected_concatenation(SEGMENT_COUNT), "byte-identical after resume");
    assert!(
        origin.requests.load(Ordering::SeqCst) - requests_before < 20,
        "a resume must not refetch chunks already on disk"
    );
    assert!(origin.peak_in_flight.load(Ordering::SeqCst) > 1, "ranges are fetched concurrently");
}

/// A CDN that serves a bounded prefix per link, then refuses: the ranged plan
/// reports how far it got rather than failing, and a fresh link for the same
/// bytes picks up exactly there — the mechanism that turns YouTube's per-link
/// allowance into a sequence of resolve-and-continue cycles.
#[tokio::test]
async fn a_refused_link_keeps_its_progress_and_a_fresh_link_continues() {
    let mut origin = Origin::new();
    origin.allowance_per_link = Some(SEGMENT_LEN * 5);
    let origin = Arc::new(origin);
    let addr = spawn_origin(Arc::clone(&origin)).await;
    let total = (SEGMENT_LEN * SEGMENT_COUNT) as u64;
    let plan = ChunkPlan { total, chunk_size: SEGMENT_LEN as u64, identity: "yt:o-abc:137:1:49152".into() };

    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("refused.mp4");
    let client = reqwest::Client::new();

    let mut resume = StreamProgress::default();
    let mut links = 0;
    loop {
        links += 1;
        let options = StreamOptions {
            resume: resume.clone(),
            segments: SegmentOptions { concurrency: 2, ..SegmentOptions::default() },
            ..StreamOptions::default()
        };
        let outcome = download_ranged_with_options(
            &client,
            &format!("http://{addr}/file.mp4?sig=link{links}"),
            &plan,
            &Vec::new(),
            &dest,
            None,
            &options,
        )
        .await
        .expect("a refusal is an outcome, not an error");
        match outcome {
            StreamOutcome::Refused { progress, status, .. } => {
                assert_eq!(status, 403);
                assert!(progress.bytes > resume.bytes, "every link must advance: {progress:?}");
                assert_eq!(progress.bytes % SEGMENT_LEN as u64, 0, "progress lands on a whole chunk");
                resume = progress;
            }
            StreamOutcome::Completed { bytes } => {
                assert_eq!(bytes, total);
                break;
            }
            StreamOutcome::Paused(_) => panic!("nothing cancelled this"),
        }
        assert!(links < 10, "too many links for a 12-chunk file");
    }
    assert!(links >= 3, "the allowance forced several links: {links}");
    assert_eq!(std::fs::read(&dest).unwrap(), expected_concatenation(SEGMENT_COUNT), "byte-identical across links");
}
