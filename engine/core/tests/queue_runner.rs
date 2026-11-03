//! Queue + runner integration: real downloads driven off SQLite state.


mod support;

use std::time::Duration;

use zoltraak_core::downloader::Options;
use zoltraak_core::model::DownloadState;
use zoltraak_core::queue::NewDownload;
use zoltraak_core::runner::RunnerConfig;
use zoltraak_core::{Downloader, Queue, Runner};
use support::{Config, TestServer, payload};
use tokio_util::sync::CancellationToken;

fn runner(queue: Queue, max_concurrent: usize) -> Runner {
    Runner::new(
        queue,
        Downloader::new(
            reqwest::Client::new(),
            Options {
                connections: Some(4),
                min_steal_bytes: 4096,
                persist_every: Duration::from_millis(50),
                ..Options::default()
            },
        ),
        RunnerConfig {
            max_concurrent,
            progress_every: Duration::from_millis(20),
            idle_poll: Duration::from_millis(20),
        },
    )
}

#[tokio::test]
async fn runner_drains_the_queue_and_writes_real_files() {
    let data = payload(300_000);
    let server = TestServer::start(Config::new(data.clone())).await;
    let dir = tempfile::tempdir().unwrap();
    let queue = Queue::open_in_memory().await.unwrap();

    // Three distinct destinations so the uniqueness constraint is satisfied.
    for name in ["a", "b", "c"] {
        queue
            .enqueue(NewDownload {
                filename: format!("{name}.bin"),
                ..NewDownload::new(server.url(), dir.path())
            })
            .await
            .unwrap();
    }

    let summary = runner(queue.clone(), 3)
        .run_until_empty(CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(summary.completed, 3, "summary: {summary:?}");
    assert_eq!(summary.failed, 0);

    for item in queue.list().await.unwrap() {
        assert_eq!(item.state, DownloadState::Finished);
        assert_eq!(item.downloaded_bytes, data.len() as u64);
        assert!(item.completed_at.is_some());
        assert_eq!(
            std::fs::read(item.path()).unwrap(),
            data,
            "{} has wrong content",
            item.filename
        );
    }
}

#[tokio::test]
async fn a_failing_item_does_not_stop_the_rest() {
    let data = payload(100_000);
    let server = TestServer::start(Config::new(data.clone())).await;
    let dir = tempfile::tempdir().unwrap();
    let queue = Queue::open_in_memory().await.unwrap();

    queue
        .enqueue(NewDownload {
            filename: "good.bin".into(),
            ..NewDownload::new(server.url(), dir.path())
        })
        .await
        .unwrap();
    queue
        .enqueue(NewDownload {
            filename: "bad.bin".into(),
            ..NewDownload::new(format!("http://{}/missing", server.addr), dir.path())
        })
        .await
        .unwrap();

    let summary = runner(queue.clone(), 2)
        .run_until_empty(CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(summary.completed, 1);
    assert_eq!(summary.failed, 1);

    let items = queue.list().await.unwrap();
    let good = items.iter().find(|i| i.filename == "good.bin").unwrap();
    let bad = items.iter().find(|i| i.filename == "bad.bin").unwrap();

    assert_eq!(good.state, DownloadState::Finished);
    assert_eq!(bad.state, DownloadState::Failed);
    assert!(bad.error.is_some(), "a failure must record why");
}

#[tokio::test]
async fn concurrency_limit_is_respected() {
    let data = payload(400_000);
    let mut config = Config::new(data);
    config.piece_delay = Duration::from_millis(3);
    config.piece_size = 16 * 1024;
    let server = TestServer::start(config).await;
    let dir = tempfile::tempdir().unwrap();
    let queue = Queue::open_in_memory().await.unwrap();

    for name in ["a", "b", "c", "d", "e", "f"] {
        queue
            .enqueue(NewDownload {
                filename: format!("{name}.bin"),
                ..NewDownload::new(server.url(), dir.path())
            })
            .await
            .unwrap();
    }

    let queue_probe = queue.clone();
    let done = CancellationToken::new();
    let watcher = {
        let done = done.clone();
        tokio::spawn(async move {
            let mut peak = 0usize;
            while !done.is_cancelled() {
                let active = queue_probe
                    .list_by_state(DownloadState::Downloading)
                    .await
                    .unwrap()
                    .len();
                peak = peak.max(active);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            peak
        })
    };

    let summary = runner(queue.clone(), 2)
        .run_until_empty(CancellationToken::new())
        .await
        .unwrap();
    done.cancel();
    let peak = watcher.await.unwrap();

    assert_eq!(summary.completed, 6);
    assert!(
        peak <= 2,
        "concurrency limit of 2 was exceeded: saw {peak} downloading at once"
    );
}

/// The M1 exit criterion, end to end: state survives a simulated restart and
/// the interrupted item resumes rather than restarting.
#[tokio::test]
async fn interrupted_run_resumes_after_restart() {
    let data = payload(3_000_000);
    let mut config = Config::new(data.clone());
    config.piece_delay = Duration::from_millis(4);
    config.piece_size = 32 * 1024;
    let server = TestServer::start(config).await;

    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("zoltraak.db");
    let downloads = dir.path().join("files");

    let id = {
        let queue = Queue::open(&db).await.unwrap();
        let id = queue
            .enqueue(NewDownload {
                filename: "big.bin".into(),
                ..NewDownload::new(server.url(), &downloads)
            })
            .await
            .unwrap();

        // Interrupt once real progress has landed.
        let cancel = CancellationToken::new();
        let probe = queue.clone();
        let trigger = {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                loop {
                    let item = probe.get(id).await.unwrap();
                    if item.downloaded_bytes > 300_000 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                cancel.cancel();
            })
        };

        runner(queue.clone(), 1).run_until_empty(cancel).await.unwrap();
        trigger.await.unwrap();

        let item = queue.get(id).await.unwrap();
        assert_eq!(item.state, DownloadState::Paused, "expected an interruption");
        assert!(item.downloaded_bytes > 0);
        assert!(
            item.downloaded_bytes < data.len() as u64,
            "finished before it could be interrupted"
        );
        queue.close().await;
        id
    };

    // A fresh process: reopen the database and run again.
    let queue = Queue::open(&db).await.unwrap();
    let partial = queue.get(id).await.unwrap().downloaded_bytes;
    assert!(partial > 0, "progress did not survive the restart");

    // Paused items are not auto-claimed, so the user re-queues it explicitly.
    queue.set_state(id, DownloadState::Waiting).await.unwrap();

    let summary = runner(queue.clone(), 1)
        .run_until_empty(CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(summary.completed, 1);
    let item = queue.get(id).await.unwrap();
    assert_eq!(item.state, DownloadState::Finished);
    assert_eq!(
        std::fs::read(item.path()).unwrap(),
        data,
        "resumed file does not match the original"
    );

    assert!(
        server.config.range_requests.load(std::sync::atomic::Ordering::Relaxed) > 0,
        "resume should have used ranged requests"
    );
}

#[tokio::test]
async fn crashed_downloading_rows_are_recovered_on_next_run() {
    let data = payload(80_000);
    let server = TestServer::start(Config::new(data.clone())).await;
    let dir = tempfile::tempdir().unwrap();
    let queue = Queue::open_in_memory().await.unwrap();

    let id = queue
        .enqueue(NewDownload {
            filename: "orphan.bin".into(),
            ..NewDownload::new(server.url(), dir.path())
        })
        .await
        .unwrap();

    // Simulate a process that died holding the claim.
    queue.set_state(id, DownloadState::Downloading).await.unwrap();

    let summary = runner(queue.clone(), 1)
        .run_until_empty(CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(
        summary.completed, 1,
        "an orphaned row must be picked back up, not stranded"
    );
    assert_eq!(std::fs::read(dir.path().join("orphan.bin")).unwrap(), data);
}
