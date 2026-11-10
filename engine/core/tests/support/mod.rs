//! A configurable HTTP origin for exercising the download engine.
//!
//! Real range semantics matter here: mocking the client would test the mock,
//! not the engine's handling of 206s, `Content-Range`, and validator drift.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::StreamExt;

/// Deterministic, position-dependent content, so a mis-ordered or duplicated
/// range shows up as a byte mismatch rather than passing by luck.
pub fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

pub struct Config {
    pub data: Vec<u8>,
    pub accept_ranges: bool,
    /// Advertise ranges on HEAD but ignore them on GET.
    pub ignore_ranges: bool,
    pub range_header: Option<String>,
    pub truncate_first: AtomicU32,
    pub fail_full_first: AtomicU32,
    pub fail_full_after_probe: bool,
    pub fail_after_ranges: Option<usize>,
    pub head_supported: bool,
    pub etag: std::sync::Mutex<Option<String>>,
    pub disposition: Option<String>,
    /// Delay inserted between body pieces, to make cancellation observable.
    pub piece_delay: Duration,
    pub piece_size: usize,
    /// Fail this many range requests with 503 before serving normally.
    pub fail_first: AtomicU32,
    pub range_requests: AtomicUsize,
    pub full_requests: AtomicUsize,
    /// This many range responses send their first piece, then go silent —
    /// a connection stuck on a bad path.
    pub stall_first: AtomicU32,
    /// This many range responses use `slow_piece_delay` instead of
    /// `piece_delay`, so one connection crawls while its peers run normally.
    pub slow_first: AtomicU32,
    pub slow_piece_delay: Duration,
    /// Refuse range requests with 503 while this many are already streaming,
    /// like hosts that limit connections per client.
    pub max_concurrent_ranges: Option<usize>,
    pub ranges_in_flight: Arc<AtomicUsize>,
}

/// Counts a range response as in flight until its body is dropped.
struct InFlight(Arc<AtomicUsize>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

fn take_one(counter: &AtomicU32) -> bool {
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
        .is_ok()
}

impl Config {
    pub fn new(data: Vec<u8>) -> Self {
        Self {
            data,
            accept_ranges: true,
            ignore_ranges: false,
            range_header: None,
            truncate_first: AtomicU32::new(0),
            fail_full_first: AtomicU32::new(0),
            fail_full_after_probe: false,
            fail_after_ranges: None,
            head_supported: true,
            etag: std::sync::Mutex::new(Some("\"v1\"".to_string())),
            disposition: None,
            piece_delay: Duration::ZERO,
            piece_size: 64 * 1024,
            fail_first: AtomicU32::new(0),
            range_requests: AtomicUsize::new(0),
            full_requests: AtomicUsize::new(0),
            stall_first: AtomicU32::new(0),
            slow_first: AtomicU32::new(0),
            slow_piece_delay: Duration::ZERO,
            max_concurrent_ranges: None,
            ranges_in_flight: Arc::new(AtomicUsize::new(0)),
        }
    }

    // Each integration test compiles its own copy of this module, so a helper
    // only one of them needs looks dead to the others.
    #[allow(dead_code)]
    pub fn set_etag(&self, value: Option<&str>) {
        *self.etag.lock().unwrap() = value.map(str::to_string);
    }
}

pub struct TestServer {
    pub addr: SocketAddr,
    // Only the tests that assert on request counters reach for this, and each
    // integration test compiles its own copy of this module.
    #[allow(dead_code)]
    pub config: Arc<Config>,
}

impl TestServer {
    pub async fn start(config: Config) -> Self {
        let config = Arc::new(config);
        let app = Router::new()
            .route("/file", get(serve))
            .with_state(Arc::clone(&config));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        Self { addr, config }
    }

    pub fn url(&self) -> String {
        format!("http://{}/file", self.addr)
    }
}

/// Parse `bytes=start-end` into inclusive bounds.
fn parse_range(value: &str, total: u64) -> Option<(u64, u64)> {
    let spec = value.strip_prefix("bytes=")?;
    let (start, end) = spec.split_once('-')?;
    let start: u64 = start.trim().parse().ok()?;
    let end = match end.trim() {
        "" => total - 1,
        v => v.parse().ok()?,
    };
    (start <= end && start < total).then(|| (start, end.min(total - 1)))
}

async fn serve(
    State(cfg): State<Arc<Config>>,
    headers: HeaderMap,
    method: axum::http::Method,
) -> Response {
    if method == axum::http::Method::HEAD && !cfg.head_supported {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }

    let total = cfg.data.len() as u64;
    let range = headers
        .get(axum::http::header::RANGE)
        .and_then(|v| v.to_str().ok())
        .filter(|_| cfg.accept_ranges && !cfg.ignore_ranges)
        .and_then(|v| parse_range(v, total));

    let mut response_headers = Vec::new();
    if cfg.accept_ranges {
        response_headers.push((
            axum::http::header::ACCEPT_RANGES,
            HeaderValue::from_static("bytes"),
        ));
    }
    if let Some(etag) = cfg.etag.lock().unwrap().as_deref() {
        response_headers.push((
            axum::http::header::ETAG,
            HeaderValue::from_str(etag).unwrap(),
        ));
    }
    if let Some(d) = &cfg.disposition {
        response_headers.push((
            axum::http::header::CONTENT_DISPOSITION,
            HeaderValue::from_str(d).unwrap(),
        ));
    }

    let (status, slice) = match range {
        Some((start, end)) => {
            let count = cfg.range_requests.fetch_add(1, Ordering::Relaxed);
            if cfg.fail_after_ranges.is_some_and(|limit| count >= limit) {
                return StatusCode::FORBIDDEN.into_response();
            }

            if cfg
                .fail_first
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
                .is_ok()
            {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            if cfg
                .max_concurrent_ranges
                .is_some_and(|limit| cfg.ranges_in_flight.load(Ordering::Relaxed) >= limit)
            {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }

            response_headers.push((
                axum::http::header::CONTENT_RANGE,
                HeaderValue::from_str(
                    &cfg.range_header
                        .clone()
                        .unwrap_or_else(|| format!("bytes {start}-{end}/{total}")),
                )
                .unwrap(),
            ));
            let mut slice = cfg.data[start as usize..=end as usize].to_vec();
            if cfg
                .truncate_first
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
                .is_ok()
            {
                slice.truncate(slice.len() / 2);
            }
            (StatusCode::PARTIAL_CONTENT, slice)
        }
        None => {
            let count = cfg.full_requests.fetch_add(1, Ordering::Relaxed);
            if method != axum::http::Method::HEAD
                && (!cfg.fail_full_after_probe || count >= 2)
                && cfg
                    .fail_full_first
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
                    .is_ok()
            {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            (StatusCode::OK, cfg.data.clone())
        }
    };

    let len = slice.len();
    let mut builder = Response::builder().status(status).header(
        axum::http::header::CONTENT_LENGTH,
        HeaderValue::from_str(&len.to_string()).unwrap(),
    );
    for (name, value) in response_headers {
        builder = builder.header::<HeaderName, HeaderValue>(name, value);
    }

    // HEAD must carry the headers but no body.
    if method == axum::http::Method::HEAD {
        return builder.body(Body::empty()).unwrap();
    }

    let ranged = status == StatusCode::PARTIAL_CONTENT;
    let stall = ranged && take_one(&cfg.stall_first);
    let delay = if ranged && take_one(&cfg.slow_first) {
        cfg.slow_piece_delay
    } else {
        cfg.piece_delay
    };
    let in_flight = ranged.then(|| {
        cfg.ranges_in_flight.fetch_add(1, Ordering::Relaxed);
        Arc::new(InFlight(Arc::clone(&cfg.ranges_in_flight)))
    });
    let pieces: Vec<Bytes> = slice
        .chunks(cfg.piece_size.max(1))
        .map(Bytes::copy_from_slice)
        .collect();

    let stream = futures_util::stream::iter(pieces.into_iter().enumerate()).then(move |(index, piece)| {
        let in_flight = in_flight.clone();
        async move {
            let _held = in_flight;
            if stall && index > 0 {
                std::future::pending::<()>().await;
            }
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            Ok::<_, Infallible>(piece)
        }
    });

    builder.body(Body::from_stream(stream)).unwrap()
}
