//! End-to-end engine tests against a real HTTP origin.


mod support;

use std::sync::atomic::Ordering;
use std::time::Duration;

use support::{Config, TestServer, payload};
use tokio_util::sync::CancellationToken;
use zoltraak_core::downloader::{Options, Outcome, Request};
use zoltraak_core::resume::ResumeState;
use zoltraak_core::{DownloadId, Downloader, progress_channel};

fn downloader(connections: u8) -> Downloader {
    Downloader::new(
        reqwest::Client::new(),
        Options {
            connections: Some(connections),
            // Small enough that stealing actually triggers on test-sized files.
            min_steal_bytes: 4096,
            persist_every: Duration::from_millis(50),
            retry: zoltraak_core::retry::RetryPolicy {
                base: Duration::from_millis(5),
                max_delay: Duration::from_millis(20),
                max_attempts: 3,
            },
            ..Options::default()
        },
    )
}

#[tokio::test]
async fn segmented_download_is_byte_identical() {
    let data = payload(1_500_000);
    let server = TestServer::start(Config::new(data.clone())).await;
    let dir = tempfile::tempdir().unwrap();

    let (tx, _rx) = progress_channel(DownloadId::new());
    let outcome = downloader(8)
        .download(
            &Request::new(server.url(), dir.path()),
            CancellationToken::new(),
            &tx,
        )
        .await
        .expect("download should succeed");

    let Outcome::Completed { path, bytes } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };

    assert_eq!(bytes, data.len() as u64);
    assert_eq!(std::fs::read(&path).unwrap(), data, "content mismatch");
    assert!(
        server.config.range_requests.load(Ordering::Relaxed) >= 8,
        "expected at least one ranged request per connection"
    );
    assert!(
        !ResumeState::sidecar_path(&path).exists(),
        "sidecar must be cleaned up after success"
    );
}

#[tokio::test]
async fn single_connection_matches_multi_connection_output() {
    let data = payload(300_000);
    let server = TestServer::start(Config::new(data.clone())).await;
    let dir = tempfile::tempdir().unwrap();

    let (tx, _rx) = progress_channel(DownloadId::new());
    downloader(1)
        .download(
            &Request::new(server.url(), dir.path()),
            CancellationToken::new(),
            &tx,
        )
        .await
        .unwrap();

    assert_eq!(std::fs::read(dir.path().join("file")).unwrap(), data);
}

#[tokio::test]
async fn server_without_range_support_falls_back_to_one_stream() {
    let data = payload(200_000);
    let mut config = Config::new(data.clone());
    config.accept_ranges = false;
    let server = TestServer::start(config).await;
    let dir = tempfile::tempdir().unwrap();

    let (tx, _rx) = progress_channel(DownloadId::new());
    let outcome = downloader(8)
        .download(
            &Request::new(server.url(), dir.path()),
            CancellationToken::new(),
            &tx,
        )
        .await
        .unwrap();

    let Outcome::Completed { path, .. } = outcome else {
        panic!("expected completion");
    };
    assert_eq!(std::fs::read(&path).unwrap(), data);
    assert_eq!(
        server.config.range_requests.load(Ordering::Relaxed),
        0,
        "must not issue ranged requests to a server that cannot honour them"
    );
}

#[tokio::test]
async fn head_rejection_still_detects_range_support() {
    // Servers that 405 on HEAD must not be downgraded to a single connection.
    let data = payload(600_000);
    let mut config = Config::new(data.clone());
    config.head_supported = false;
    let server = TestServer::start(config).await;
    let dir = tempfile::tempdir().unwrap();

    let (tx, _rx) = progress_channel(DownloadId::new());
    downloader(4)
        .download(
            &Request::new(server.url(), dir.path()),
            CancellationToken::new(),
            &tx,
        )
        .await
        .unwrap();

    assert_eq!(std::fs::read(dir.path().join("file")).unwrap(), data);
    assert!(
        server.config.range_requests.load(Ordering::Relaxed) >= 4,
        "range support should have been detected via the GET fallback"
    );
}

/// The M0 exit criterion: interrupt mid-flight, resume, get the same bytes.
#[tokio::test]
async fn cancelled_download_resumes_to_identical_bytes() {
    let data = payload(4_000_000);
    let mut config = Config::new(data.clone());
    // Slow the origin so cancellation lands mid-download rather than after it.
    config.piece_delay = Duration::from_millis(4);
    config.piece_size = 32 * 1024;
    let server = TestServer::start(config).await;
    let dir = tempfile::tempdir().unwrap();
    let request = Request::new(server.url(), dir.path());

    let cancel = CancellationToken::new();
    let (tx, mut rx) = progress_channel(DownloadId::new());

    let watcher = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            // Cancel once a real but partial amount has landed.
            while rx.changed().await.is_ok() {
                if rx.borrow().downloaded_bytes > 500_000 {
                    break;
                }
            }
            cancel.cancel();
        })
    };

    let first = downloader(8)
        .download(&request, cancel, &tx)
        .await
        .expect("cancellation is not an error");
    watcher.await.unwrap();

    let Outcome::Paused {
        path, downloaded, ..
    } = first
    else {
        panic!("expected a pause, got {first:?} — the origin was too fast to interrupt");
    };

    assert!(downloaded > 0, "nothing was downloaded before cancelling");
    assert!(
        downloaded < data.len() as u64,
        "download completed before it could be cancelled"
    );
    assert!(
        ResumeState::sidecar_path(&path).exists(),
        "pausing must leave resume state behind"
    );

    // Resume with a fresh downloader, as a process restart would.
    let (tx2, _rx2) = progress_channel(DownloadId::new());
    let second = downloader(8)
        .download(&request, CancellationToken::new(), &tx2)
        .await
        .expect("resume should succeed");

    let Outcome::Completed { path, bytes } = second else {
        panic!("expected completion on resume, got {second:?}");
    };

    assert_eq!(bytes, data.len() as u64);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        data,
        "resumed file does not match the original"
    );
    assert!(!ResumeState::sidecar_path(&path).exists());
}

#[tokio::test]
async fn changed_remote_file_restarts_instead_of_splicing() {
    let data = payload(2_000_000);
    let mut config = Config::new(data.clone());
    config.piece_delay = Duration::from_millis(4);
    config.piece_size = 32 * 1024;
    let server = TestServer::start(config).await;
    let dir = tempfile::tempdir().unwrap();
    let request = Request::new(server.url(), dir.path());

    // Interrupt partway.
    let cancel = CancellationToken::new();
    let (tx, mut rx) = progress_channel(DownloadId::new());
    let watcher = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            while rx.changed().await.is_ok() {
                if rx.borrow().downloaded_bytes > 200_000 {
                    break;
                }
            }
            cancel.cancel();
        })
    };
    let paused = downloader(4).download(&request, cancel, &tx).await.unwrap();
    watcher.await.unwrap();
    assert!(matches!(paused, Outcome::Paused { .. }), "expected a pause");

    // The origin now serves different content under a new validator.
    let replacement: Vec<u8> = payload(2_000_000).iter().map(|b| b ^ 0xFF).collect();
    let server2 = {
        let cfg = Config::new(replacement.clone());
        cfg.set_etag(Some("\"v2\""));
        TestServer::start(cfg).await
    };

    // Point the same destination at the changed resource by reusing the dir;
    // the sidecar from the first attempt is still present.
    let request2 = Request {
        url: server2.url(),
        directory: dir.path().to_path_buf(),
        filename: Some("file".into()),
        headers: Vec::new(),
    };
    let (tx2, _rx2) = progress_channel(DownloadId::new());
    let outcome = downloader(4)
        .download(&request2, CancellationToken::new(), &tx2)
        .await
        .unwrap();

    let Outcome::Completed { path, .. } = outcome else {
        panic!("expected completion");
    };
    assert_eq!(
        std::fs::read(&path).unwrap(),
        replacement,
        "stale ranges were spliced into the new file"
    );
}

#[tokio::test]
async fn transient_failures_are_retried() {
    let data = payload(400_000);
    let mut config = Config::new(data.clone());
    config.fail_first = std::sync::atomic::AtomicU32::new(3);
    let server = TestServer::start(config).await;
    let dir = tempfile::tempdir().unwrap();

    let downloader = Downloader::new(
        reqwest::Client::new(),
        Options {
            connections: Some(4),
            retry: zoltraak_core::retry::RetryPolicy {
                max_attempts: 5,
                base: Duration::from_millis(10),
                max_delay: Duration::from_millis(50),
            },
            min_steal_bytes: 4096,
            persist_every: Duration::from_millis(50),
            // Disable proactive scaling so this test is purely about retry logic.
            scaler_interval: Duration::MAX,
        },
    );

    let (tx, _rx) = progress_channel(DownloadId::new());
    let outcome = downloader
        .download(
            &Request::new(server.url(), dir.path()),
            CancellationToken::new(),
            &tx,
        )
        .await
        .expect("503s should be retried, not fatal");

    let Outcome::Completed { path, .. } = outcome else {
        panic!("expected completion");
    };
    assert_eq!(std::fs::read(&path).unwrap(), data);
}

#[tokio::test]
async fn filename_comes_from_content_disposition() {
    let data = payload(50_000);
    let mut config = Config::new(data);
    config.disposition = Some(r#"attachment; filename="report final.pdf""#.into());
    let server = TestServer::start(config).await;
    let dir = tempfile::tempdir().unwrap();

    let (tx, _rx) = progress_channel(DownloadId::new());
    let outcome = downloader(4)
        .download(
            &Request::new(server.url(), dir.path()),
            CancellationToken::new(),
            &tx,
        )
        .await
        .unwrap();

    let Outcome::Completed { path, .. } = outcome else {
        panic!("expected completion");
    };
    assert_eq!(path.file_name().unwrap(), "report final.pdf");
}

#[tokio::test]
async fn progress_reaches_the_full_size() {
    let data = payload(800_000);
    let server = TestServer::start(Config::new(data.clone())).await;
    let dir = tempfile::tempdir().unwrap();

    let (tx, rx) = progress_channel(DownloadId::new());
    downloader(4)
        .download(
            &Request::new(server.url(), dir.path()),
            CancellationToken::new(),
            &tx,
        )
        .await
        .unwrap();

    let final_progress = *rx.borrow();
    assert_eq!(final_progress.downloaded_bytes, data.len() as u64);
    assert_eq!(final_progress.total_bytes, Some(data.len() as u64));
    assert_eq!(final_progress.fraction(), Some(1.0));
}

#[tokio::test]
async fn more_connections_than_bytes_does_not_corrupt() {
    // 32 connections over a 10-byte file must not produce empty or overlapping
    // ranges.
    let data = payload(10);
    let server = TestServer::start(Config::new(data.clone())).await;
    let dir = tempfile::tempdir().unwrap();

    let (tx, _rx) = progress_channel(DownloadId::new());
    downloader(32)
        .download(
            &Request::new(server.url(), dir.path()),
            CancellationToken::new(),
            &tx,
        )
        .await
        .unwrap();

    assert_eq!(std::fs::read(dir.path().join("file")).unwrap(), data);
}

/// A bandwidth cap must be a genuine floor on wall-clock time, not just a
/// number stored somewhere. Asserts a lower bound rather than an upper one —
/// "at least this slow" cannot flake on a loaded CI box the way "at most this
/// fast" can.
#[tokio::test]
async fn a_bandwidth_limit_measurably_slows_a_download() {
    let bytes_per_second = 40_000u64;
    let size = 200_000usize; // 5s of budget at the cap above
    let data = payload(size);
    let server = TestServer::start(Config::new(data.clone())).await;
    let dir = tempfile::tempdir().unwrap();

    let dl = downloader(4);
    dl.set_bandwidth_limit(Some(bytes_per_second)).await;

    let (tx, _rx) = progress_channel(DownloadId::new());
    let start = std::time::Instant::now();
    dl.download(
        &Request::new(server.url(), dir.path()),
        CancellationToken::new(),
        &tx,
    )
    .await
    .unwrap();
    let elapsed = start.elapsed();

    // The bucket starts full (one second's worth of free burst), so the true
    // floor is (size - one second of bytes) / rate, not size / rate. A
    // generous tolerance below that still leaves a wide margin over what an
    // unthrottled transfer takes (well under 100ms on loopback).
    let theoretical_minimum =
        Duration::from_secs_f64((size as f64 - bytes_per_second as f64) / bytes_per_second as f64);
    assert!(
        elapsed >= theoretical_minimum.mul_f64(0.7),
        "download finished in {elapsed:?}, which is faster than the {bytes_per_second} B/s cap allows"
    );
    assert_eq!(std::fs::read(dir.path().join("file")).unwrap(), data);
}

#[tokio::test]
async fn lifting_a_bandwidth_limit_takes_effect_immediately() {
    let data = payload(50_000);
    let server = TestServer::start(Config::new(data.clone())).await;
    let dir = tempfile::tempdir().unwrap();

    let dl = downloader(4);
    dl.set_bandwidth_limit(Some(1)).await; // effectively frozen
    dl.set_bandwidth_limit(None).await; // lifted before the download starts

    let (tx, _rx) = progress_channel(DownloadId::new());
    let start = std::time::Instant::now();
    dl.download(
        &Request::new(server.url(), dir.path()),
        CancellationToken::new(),
        &tx,
    )
    .await
    .unwrap();

    assert!(
        start.elapsed() < Duration::from_secs(5),
        "an unlimited download should not have inherited the earlier cap"
    );
}

#[tokio::test]
async fn advertised_but_ignored_ranges_restart_with_identical_bytes() {
    let data = payload(250_000);
    let mut config = Config::new(data.clone());
    config.ignore_ranges = true;
    let server = TestServer::start(config).await;
    let dir = tempfile::tempdir().unwrap();
    // Old segmented checkpoints must not survive a single-stream restart.
    let path = dir.path().join("file");
    std::fs::write(&path, vec![0; data.len()]).unwrap();
    let mut chunk = zoltraak_core::chunk::Chunk::new(0, data.len() as u64);
    chunk.downloaded = 100;
    ResumeState {
        url: server.url(),
        total: data.len() as u64,
        validator: Some("\"v1\"".into()),
        chunks: vec![chunk],
    }
    .save(&path)
    .unwrap();
    let (tx, rx) = progress_channel(DownloadId::new());
    let cancel = CancellationToken::new();
    let result = downloader(4)
        .download(&Request::new(server.url(), dir.path()), cancel.clone(), &tx)
        .await
        .unwrap();
    assert!(matches!(result, Outcome::Completed { .. }));
    assert!(
        !cancel.is_cancelled(),
        "an internal fallback must not cancel the user's token"
    );
    assert_eq!(std::fs::read(&path).unwrap(), data);
    assert!(!ResumeState::sidecar_path(&path).exists());
    assert_eq!(rx.borrow().active_connections, 0);
}

#[tokio::test]
async fn malformed_or_wrong_range_responses_restart_safely() {
    for range in [
        "garbage",
        "bytes 1-2/250000",
        "bytes 0-249999/999999",
        "bytes 0-999999/250000",
    ] {
        let data = payload(250_000);
        let mut config = Config::new(data.clone());
        config.range_header = Some(range.into());
        let server = TestServer::start(config).await;
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = progress_channel(DownloadId::new());
        let result = downloader(4)
            .download(
                &Request::new(server.url(), dir.path()),
                CancellationToken::new(),
                &tx,
            )
            .await
            .unwrap();
        assert!(matches!(result, Outcome::Completed { .. }));
        assert_eq!(
            std::fs::read(dir.path().join("file")).unwrap(),
            data,
            "range: {range}"
        );
    }
}

#[tokio::test]
async fn interrupted_ranges_retry_from_the_written_offset() {
    let data = payload(250_000);
    let config = Config::new(data.clone());
    config.truncate_first.store(2, Ordering::Relaxed);
    let server = TestServer::start(config).await;
    let dir = tempfile::tempdir().unwrap();
    let (tx, _rx) = progress_channel(DownloadId::new());
    downloader(1)
        .download(
            &Request::new(server.url(), dir.path()),
            CancellationToken::new(),
            &tx,
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read(dir.path().join("file")).unwrap(), data);
    assert!(server.config.range_requests.load(Ordering::Relaxed) >= 3);
}

#[tokio::test]
async fn persistent_empty_ranges_exhaust_retry_budget_and_save_resume() {
    let config = Config::new(payload(1));
    config.truncate_first.store(100, Ordering::Relaxed);
    let server = TestServer::start(config).await;
    let dir = tempfile::tempdir().unwrap();
    let (tx, _rx) = progress_channel(DownloadId::new());
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        downloader(1).download(
            &Request::new(server.url(), dir.path()),
            CancellationToken::new(),
            &tx,
        ),
    )
    .await
    .unwrap();
    assert!(matches!(
        result,
        Err(zoltraak_core::Error::IncompleteResponse)
    ));
    assert_eq!(server.config.range_requests.load(Ordering::Relaxed), 4);
    assert!(ResumeState::load(&dir.path().join("file")).is_some());
}

#[tokio::test]
async fn single_stream_and_probe_retry_transient_failures() {
    for after_probe in [false, true] {
        let data = payload(150_000);
        let mut config = Config::new(data.clone());
        config.accept_ranges = false;
        config.fail_full_after_probe = after_probe;
        config.fail_full_first.store(2, Ordering::Relaxed);
        let server = TestServer::start(config).await;
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = progress_channel(DownloadId::new());
        downloader(4)
            .download(
                &Request::new(server.url(), dir.path()),
                CancellationToken::new(),
                &tx,
            )
            .await
            .unwrap();
        assert_eq!(std::fs::read(dir.path().join("file")).unwrap(), data);
    }
}

#[tokio::test]
async fn stalled_response_can_be_paused_without_waiting_for_a_body_piece() {
    for accepts_ranges in [true, false] {
        let mut config = Config::new(payload(150_000));
        config.accept_ranges = accepts_ranges;
        config.piece_delay = Duration::from_secs(120);
        let server = TestServer::start(config).await;
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            stop.cancel();
        });
        let (tx, _rx) = progress_channel(DownloadId::new());
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            downloader(4).download(&Request::new(server.url(), dir.path()), cancel, &tx),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(matches!(result, Outcome::Paused { .. }));
    }
}

#[tokio::test]
async fn dynamically_added_workers_propagate_errors_and_are_joined() {
    let data = payload(2_000_000);
    let mut config = Config::new(data.clone());
    config.piece_size = 16_384;
    config.piece_delay = Duration::from_millis(10);
    config.fail_after_ranges = Some(1);
    let server = TestServer::start(config).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    std::fs::write(&path, vec![0; data.len()]).unwrap();
    ResumeState {
        url: server.url(),
        total: data.len() as u64,
        validator: Some("\"v1\"".into()),
        chunks: vec![zoltraak_core::chunk::Chunk::new(0, data.len() as u64)],
    }
    .save(&path)
    .unwrap();
    let dl = Downloader::new(
        reqwest::Client::new(),
        Options {
            connections: Some(4),
            min_steal_bytes: 100_000,
            scaler_interval: Duration::from_millis(25),
            ..Options::default()
        },
    );
    let (tx, _rx) = progress_channel(DownloadId::new());
    let cancel = CancellationToken::new();
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        dl.download(&Request::new(server.url(), dir.path()), cancel.clone(), &tx),
    )
    .await
    .unwrap();
    assert!(matches!(
        result,
        Err(zoltraak_core::Error::UnexpectedStatus { status: 403, .. })
    ));
    assert!(!cancel.is_cancelled());
    assert!(ResumeState::load(&path).is_some());
    let bytes = std::fs::read(&path).unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(
        std::fs::read(&path).unwrap(),
        bytes,
        "no worker may write after returning"
    );
}

#[tokio::test]
async fn many_saved_chunks_respect_the_new_connection_limit() {
    let data = payload(800_000);
    let mut config = Config::new(data.clone());
    config.piece_delay = Duration::from_millis(2);
    let server = TestServer::start(config).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    std::fs::write(&path, vec![0; data.len()]).unwrap();
    ResumeState {
        url: server.url(),
        total: data.len() as u64,
        validator: Some("\"v1\"".into()),
        chunks: zoltraak_core::chunk::plan(data.len() as u64, 40),
    }
    .save(&path)
    .unwrap();
    let (tx, mut rx) = progress_channel(DownloadId::new());
    let monitor = tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            assert!(
                rx.borrow().active_connections <= 2,
                "resume exceeded the configured connection limit"
            );
        }
    });
    downloader(2)
        .download(
            &Request::new(server.url(), dir.path()),
            CancellationToken::new(),
            &tx,
        )
        .await
        .unwrap();
    drop(tx);
    monitor.await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), data);
}

#[tokio::test]
async fn truncated_partial_file_is_restarted_even_with_a_matching_validator() {
    let data = payload(250_000);
    let server = TestServer::start(Config::new(data.clone())).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    std::fs::write(&path, b"truncated").unwrap();
    let mut chunk = zoltraak_core::chunk::Chunk::new(0, data.len() as u64);
    chunk.downloaded = data.len() as u64;
    ResumeState {
        url: server.url(),
        total: data.len() as u64,
        validator: Some("\"v1\"".into()),
        chunks: vec![chunk],
    }
    .save(&path)
    .unwrap();
    let (tx, _rx) = progress_channel(DownloadId::new());
    downloader(4)
        .download(
            &Request::new(server.url(), dir.path()),
            CancellationToken::new(),
            &tx,
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), data);
}

#[tokio::test]
async fn range_probe_rejection_can_still_download_an_empty_file() {
    let app = axum::Router::new().route(
        "/empty",
        axum::routing::get(
            |headers: axum::http::HeaderMap, method: axum::http::Method| async move {
                if method == axum::http::Method::HEAD {
                    axum::http::StatusCode::METHOD_NOT_ALLOWED
                } else if headers.contains_key(axum::http::header::RANGE) {
                    axum::http::StatusCode::RANGE_NOT_SATISFIABLE
                } else {
                    axum::http::StatusCode::OK
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/empty", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let (tx, _rx) = progress_channel(DownloadId::new());
    let result = downloader(4)
        .download(
            &Request::new(url, dir.path()),
            CancellationToken::new(),
            &tx,
        )
        .await
        .unwrap();
    assert!(matches!(result, Outcome::Completed { bytes: 0, .. }));
    assert_eq!(
        std::fs::metadata(dir.path().join("empty")).unwrap().len(),
        0
    );
}
