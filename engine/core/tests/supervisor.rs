//! Supervisor integration: the long-lived engine the desktop app drives.
//!
//! These exercise the parts a UI depends on and the CLI never touched —
//! per-item pause, removal mid-flight, and the shutdown/pause distinction.


mod support;

#[tokio::test]
async fn locate_and_redownload_a_missing_completed_file() {
    let data = payload(48_000);
    let server = TestServer::start(Config::new(data.clone())).await;
    let dir = tempfile::tempdir().unwrap();
    let sup = supervisor(Queue::open_in_memory().await.unwrap(), 1);
    let id = sup.enqueue(NewDownload::new(server.url(), dir.path())).await.unwrap();
    wait_for(&sup, id, "completion", |s| s.state == DownloadState::Finished).await;
    let original = sup.queue().get(id).await.unwrap().path();
    assert!(sup.restart_missing(id).await.is_err(), "existing output must not be overwritten");
    let moved = dir.path().join("moved.bin");
    std::fs::rename(&original, &moved).unwrap();
    sup.queue().locate_file(id, &moved).await.unwrap();
    assert_eq!(sup.queue().get(id).await.unwrap().path(), moved);
    assert_eq!(std::fs::read(&moved).unwrap(), data);
    assert!(sup.queue().locate_file(id, dir.path()).await.is_err());
    assert_eq!(sup.queue().get(id).await.unwrap().path(), moved);
    std::fs::remove_file(&moved).unwrap();
    sup.restart_missing(id).await.unwrap();
    wait_for(&sup, id, "download again", |s| s.state == DownloadState::Finished).await;
    let output = sup.queue().get(id).await.unwrap().path();
    assert_eq!(std::fs::read(output).unwrap(), data);
    assert_eq!(sup.snapshot().await.unwrap().len(), 1);
    sup.shutdown().await;
}

use std::time::Duration;

use chrono::Timelike;
use zoltraak_core::downloader::Options;
use zoltraak_core::model::{DownloadId, DownloadState};
use zoltraak_core::queue::NewDownload;
use zoltraak_core::runner::RunnerConfig;
use zoltraak_core::{CompletionAction, Downloader, EngineSettings, Queue, Supervisor, TimeWindow};
use support::{Config, TestServer, payload};

/// Minutes since local midnight, matching how the supervisor reads the clock.
fn now_minutes() -> u16 {
    let now = chrono::Local::now();
    (now.hour() * 60 + now.minute()) as u16
}

/// A window guaranteed closed right now: two minutes wide, starting five
/// minutes from now, so it never straddles the moment the test runs.
fn closed_window() -> TimeWindow {
    let start = (now_minutes() + 5) % 1440;
    TimeWindow {
        enabled: true,
        start_minutes: start,
        stop_minutes: (start + 2) % 1440,
    }
}

fn supervisor(queue: Queue, max_concurrent: usize) -> Supervisor {
    Supervisor::start(
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

/// Poll until `predicate` holds, so tests never depend on a fixed sleep.
async fn wait_for<F>(sup: &Supervisor, id: DownloadId, label: &str, predicate: F)
where
    F: Fn(&zoltraak_core::DownloadSnapshot) -> bool,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let snapshot = sup.snapshot().await.unwrap();
        if let Some(item) = snapshot.iter().find(|s| s.id == id.to_string()) {
            if predicate(item) {
                return;
            }
            if tokio::time::Instant::now() > deadline {
                panic!("timed out waiting for {label}; last seen: {item:?}");
            }
        } else if tokio::time::Instant::now() > deadline {
            panic!("timed out waiting for {label}; item is gone");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A server slow enough that a download is reliably still running when the
/// test reaches in to pause or remove it.
fn slow(data: Vec<u8>) -> Config {
    Config {
        piece_size: 8192,
        piece_delay: Duration::from_millis(12),
        ..Config::new(data)
    }
}

#[tokio::test]
async fn enqueued_work_starts_without_being_asked() {
    let data = payload(200_000);
    let server = TestServer::start(Config::new(data.clone())).await;
    let dir = tempfile::tempdir().unwrap();
    let sup = supervisor(Queue::open_in_memory().await.unwrap(), 2);

    let id = sup
        .enqueue(NewDownload::new(server.url(), dir.path()))
        .await
        .unwrap();

    wait_for(&sup, id, "completion", |s| {
        s.state == DownloadState::Finished
    })
    .await;

    assert_eq!(std::fs::read(dir.path().join("file")).unwrap(), data);
    sup.shutdown().await;
}

#[tokio::test]
async fn pause_stops_the_download_and_it_stays_stopped() {
    let server = TestServer::start(slow(payload(2_000_000))).await;
    let dir = tempfile::tempdir().unwrap();
    let sup = supervisor(Queue::open_in_memory().await.unwrap(), 2);

    let id = sup
        .enqueue(NewDownload::new(server.url(), dir.path()))
        .await
        .unwrap();

    wait_for(&sup, id, "bytes to start moving", |s| s.downloaded_bytes > 0).await;
    sup.pause(id).await.unwrap();
    wait_for(&sup, id, "paused", |s| s.state == DownloadState::Paused).await;

    let settled = sup.snapshot().await.unwrap()[0].downloaded_bytes;
    assert!(settled > 0, "a pause must keep the bytes already fetched");

    // The supervisor keeps looping, so this is the real assertion: a paused
    // item must not be silently reclaimed on the next pass.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let after = &sup.snapshot().await.unwrap()[0];
    assert_eq!(after.state, DownloadState::Paused);
    assert_eq!(after.bytes_per_second, 0, "a paused item has no live rate");

    sup.shutdown().await;
}

#[tokio::test]
async fn resume_after_pause_produces_the_correct_file() {
    let data = payload(2_000_000);
    let server = TestServer::start(slow(data.clone())).await;
    let dir = tempfile::tempdir().unwrap();
    let sup = supervisor(Queue::open_in_memory().await.unwrap(), 2);

    let id = sup
        .enqueue(NewDownload::new(server.url(), dir.path()))
        .await
        .unwrap();

    wait_for(&sup, id, "bytes to start moving", |s| s.downloaded_bytes > 0).await;
    sup.pause(id).await.unwrap();
    wait_for(&sup, id, "paused", |s| s.state == DownloadState::Paused).await;

    let partial = sup.snapshot().await.unwrap()[0].downloaded_bytes;
    assert!(partial < data.len() as u64, "test needs a real mid-download pause");

    sup.resume(id).await.unwrap();
    wait_for(&sup, id, "completion", |s| {
        s.state == DownloadState::Finished
    })
    .await;

    assert_eq!(
        std::fs::read(dir.path().join("file")).unwrap(),
        data,
        "resuming must not duplicate or skip a byte"
    );
    sup.shutdown().await;
}

#[tokio::test]
async fn removing_a_running_download_drops_it_from_the_queue() {
    let server = TestServer::start(slow(payload(2_000_000))).await;
    let dir = tempfile::tempdir().unwrap();
    let sup = supervisor(Queue::open_in_memory().await.unwrap(), 2);

    let id = sup
        .enqueue(NewDownload::new(server.url(), dir.path()))
        .await
        .unwrap();
    wait_for(&sup, id, "bytes to start moving", |s| s.downloaded_bytes > 0).await;

    sup.remove(id).await.unwrap();
    assert!(sup.snapshot().await.unwrap().is_empty());

    // The worker's own closing writes must not resurrect the deleted row.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        sup.snapshot().await.unwrap().is_empty(),
        "a cancelled worker wrote back a row the user deleted"
    );

    sup.shutdown().await;
}

/// The distinction that makes closing the app safe: shutting down is not the
/// same decision as pausing, so it must not persist as one.
#[tokio::test]
async fn shutdown_leaves_in_flight_work_waiting_not_paused() {
    let server = TestServer::start(slow(payload(2_000_000))).await;
    let dir = tempfile::tempdir().unwrap();
    let queue = Queue::open_in_memory().await.unwrap();
    let sup = supervisor(queue.clone(), 2);

    let id = sup
        .enqueue(NewDownload::new(server.url(), dir.path()))
        .await
        .unwrap();
    wait_for(&sup, id, "bytes to start moving", |s| s.downloaded_bytes > 0).await;

    sup.shutdown().await;

    let item = queue.get(id).await.unwrap();
    assert_eq!(
        item.state,
        DownloadState::Waiting,
        "a download interrupted by app exit should resume on the next launch"
    );
    assert!(item.downloaded_bytes > 0, "progress must survive shutdown");
}

#[tokio::test]
async fn an_explicit_pause_survives_shutdown() {
    let server = TestServer::start(slow(payload(2_000_000))).await;
    let dir = tempfile::tempdir().unwrap();
    let queue = Queue::open_in_memory().await.unwrap();
    let sup = supervisor(queue.clone(), 2);

    let id = sup
        .enqueue(NewDownload::new(server.url(), dir.path()))
        .await
        .unwrap();
    wait_for(&sup, id, "bytes to start moving", |s| s.downloaded_bytes > 0).await;

    sup.pause(id).await.unwrap();
    wait_for(&sup, id, "paused", |s| s.state == DownloadState::Paused).await;
    sup.shutdown().await;

    assert_eq!(
        queue.get(id).await.unwrap().state,
        DownloadState::Paused,
        "shutdown must not undo a pause the user asked for"
    );
}

#[tokio::test]
async fn pausing_an_item_that_has_not_started_yet_still_works() {
    let server = TestServer::start(slow(payload(2_000_000))).await;
    let dir = tempfile::tempdir().unwrap();
    // One slot, so the second item is still `waiting` when it is paused.
    let sup = supervisor(Queue::open_in_memory().await.unwrap(), 1);

    let first = sup
        .enqueue(NewDownload::new(server.url(), dir.path()))
        .await
        .unwrap();
    let second = sup
        .enqueue(NewDownload {
            filename: "second.bin".into(),
            ..NewDownload::new(server.url(), dir.path())
        })
        .await
        .unwrap();

    wait_for(&sup, first, "the first item to occupy the slot", |s| {
        s.downloaded_bytes > 0
    })
    .await;

    sup.pause(second).await.unwrap();
    wait_for(&sup, second, "the queued item to pause", |s| {
        s.state == DownloadState::Paused
    })
    .await;

    sup.shutdown().await;
}

#[tokio::test]
async fn snapshot_reports_a_live_rate_while_downloading() {
    let server = TestServer::start(slow(payload(2_000_000))).await;
    let dir = tempfile::tempdir().unwrap();
    let sup = supervisor(Queue::open_in_memory().await.unwrap(), 2);

    let id = sup
        .enqueue(NewDownload::new(server.url(), dir.path()))
        .await
        .unwrap();

    // Rate and ETA are held in memory, never in SQLite; this is what proves
    // the in-process observer is actually wired to the UI-facing snapshot.
    wait_for(&sup, id, "a live throughput reading", |s| {
        s.state == DownloadState::Downloading && s.bytes_per_second > 0
    })
    .await;

    let item = &sup.snapshot().await.unwrap()[0];
    assert!(item.total_bytes.is_some());
    assert!(item.connections >= 1);

    sup.shutdown().await;
}

/// Shutdown must terminate with more work queued than slots to run it, rather
/// than waiting for the backlog to drain.
///
/// Note this does *not* prove the related race is gone: a worker spawned in
/// the instant after a shutdown sweep would hang the wait, but the window is
/// too narrow to hit reliably from a test. It is closed structurally instead —
/// worker tokens are children of the shutdown token, so cancelling the parent
/// reaches workers that do not exist yet.
#[tokio::test]
async fn shutdown_terminates_with_a_backlog_queued() {
    let server = TestServer::start(slow(payload(4_000_000))).await;
    let dir = tempfile::tempdir().unwrap();
    let sup = supervisor(Queue::open_in_memory().await.unwrap(), 4);

    for name in ["a", "b", "c", "d", "e", "f"] {
        sup.enqueue(NewDownload {
            filename: format!("{name}.bin"),
            ..NewDownload::new(server.url(), dir.path())
        })
        .await
        .unwrap();
    }

    // No wait: shut down while the loop is still filling its slots.
    tokio::time::timeout(Duration::from_secs(15), sup.shutdown())
        .await
        .expect("shutdown hung — a worker outlived the cancellation");
}

#[tokio::test]
async fn concurrency_limit_is_respected() {
    let server = TestServer::start(slow(payload(1_000_000))).await;
    let dir = tempfile::tempdir().unwrap();
    let sup = supervisor(Queue::open_in_memory().await.unwrap(), 2);

    for name in ["a", "b", "c", "d"] {
        sup.enqueue(NewDownload {
            filename: format!("{name}.bin"),
            ..NewDownload::new(server.url(), dir.path())
        })
        .await
        .unwrap();
    }

    // Sample repeatedly: a single check could land in a gap between handoffs.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut saw_two = false;
    while tokio::time::Instant::now() < deadline {
        let running = sup
            .snapshot()
            .await
            .unwrap()
            .iter()
            .filter(|s| s.state == DownloadState::Downloading)
            .count();
        assert!(running <= 2, "{running} downloads running with a limit of 2");
        saw_two |= running == 2;

        if sup
            .snapshot()
            .await
            .unwrap()
            .iter()
            .all(|s| s.state == DownloadState::Finished)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }

    assert!(saw_two, "the limit was never actually reached");
    sup.shutdown().await;
}

#[tokio::test]
async fn a_closed_schedule_window_blocks_new_work_from_starting() {
    let server = TestServer::start(Config::new(payload(100_000))).await;
    let dir = tempfile::tempdir().unwrap();
    let queue = Queue::open_in_memory().await.unwrap();
    queue
        .save_settings(&EngineSettings {
            schedule: closed_window(),
            ..EngineSettings::default()
        })
        .await
        .unwrap();
    let sup = supervisor(queue, 2);

    let id = sup
        .enqueue(NewDownload::new(server.url(), dir.path()))
        .await
        .unwrap();

    // Give the loop several ticks to (incorrectly) start it, then confirm it
    // never did.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let state = sup.snapshot().await.unwrap()[0].state;
    assert_eq!(
        state,
        DownloadState::Waiting,
        "a download must not start outside its scheduled window"
    );

    // Reopening the schedule must let the same item start on its own.
    sup.update_settings(EngineSettings::default()).await.unwrap();
    wait_for(&sup, id, "completion once the window reopens", |s| {
        s.state == DownloadState::Finished
    })
    .await;

    sup.shutdown().await;
}

#[tokio::test]
async fn a_schedule_closing_mid_download_leaves_it_waiting_not_paused() {
    let server = TestServer::start(slow(payload(2_000_000))).await;
    let dir = tempfile::tempdir().unwrap();
    let sup = supervisor(Queue::open_in_memory().await.unwrap(), 2);

    let id = sup
        .enqueue(NewDownload::new(server.url(), dir.path()))
        .await
        .unwrap();
    wait_for(&sup, id, "bytes to start moving", |s| s.downloaded_bytes > 0).await;

    sup.update_settings(EngineSettings {
        schedule: closed_window(),
        ..EngineSettings::default()
    })
    .await
    .unwrap();

    wait_for(&sup, id, "the schedule to stop it", |s| {
        s.state == DownloadState::Waiting
    })
    .await;

    // It must not be silently reclaimed while the window stays closed.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(sup.snapshot().await.unwrap()[0].state, DownloadState::Waiting);

    sup.shutdown().await;
}

#[tokio::test]
async fn an_explicit_pause_survives_a_closed_schedule() {
    // Pausing is a user decision; a schedule closing around it must not turn
    // it back into "waiting" the way a system-initiated stop would.
    let server = TestServer::start(slow(payload(2_000_000))).await;
    let dir = tempfile::tempdir().unwrap();
    let sup = supervisor(Queue::open_in_memory().await.unwrap(), 2);

    let id = sup
        .enqueue(NewDownload::new(server.url(), dir.path()))
        .await
        .unwrap();
    wait_for(&sup, id, "bytes to start moving", |s| s.downloaded_bytes > 0).await;
    sup.pause(id).await.unwrap();
    wait_for(&sup, id, "paused", |s| s.state == DownloadState::Paused).await;

    sup.update_settings(EngineSettings {
        schedule: closed_window(),
        ..EngineSettings::default()
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(sup.snapshot().await.unwrap()[0].state, DownloadState::Paused);
    sup.shutdown().await;
}

#[tokio::test]
async fn settings_round_trip_through_the_supervisor() {
    let sup = supervisor(Queue::open_in_memory().await.unwrap(), 2);
    assert_eq!(sup.settings(), EngineSettings::default());

    let updated = EngineSettings {
        bandwidth_limit_bytes_per_sec: Some(123_456),
        default_connections: Some(5),
        ..EngineSettings::default()
    };
    sup.update_settings(updated.clone()).await.unwrap();
    assert_eq!(sup.settings(), updated);

    sup.shutdown().await;
}

#[tokio::test]
async fn an_on_complete_command_runs_once_when_the_queue_drains() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("marker.txt");

    // No quoting: `tempfile` paths never contain spaces, and quoting rules
    // differ enough between `cmd /C` and `sh -c` that it is not worth getting
    // right for a test command.
    let command = format!("echo done > {}", marker.display());

    let server = TestServer::start(Config::new(payload(50_000))).await;
    let queue = Queue::open_in_memory().await.unwrap();
    queue
        .save_settings(&EngineSettings {
            on_complete: CompletionAction::RunCommand { command },
            ..EngineSettings::default()
        })
        .await
        .unwrap();
    let sup = supervisor(queue, 2);

    let id = sup
        .enqueue(NewDownload::new(server.url(), dir.path()))
        .await
        .unwrap();
    wait_for(&sup, id, "completion", |s| s.state == DownloadState::Finished).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !marker.exists() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(marker.exists(), "on-completion command never ran");

    sup.shutdown().await;
}

#[tokio::test]
async fn file_removal_respects_choice_and_missing_outputs() {
    for (delete_file, missing) in [(false, false), (true, false), (true, true)] {
        let mut config = Config::new(payload(8_000));
        config.disposition = Some("attachment; filename=resolved.bin".into());
        let server = TestServer::start(config).await;
        let dir = tempfile::tempdir().unwrap();
        let sup = supervisor(Queue::open_in_memory().await.unwrap(), 1);
        let id = sup.enqueue(NewDownload::new(server.url(), dir.path())).await.unwrap();
        wait_for(&sup, id, "completion", |s| s.state == DownloadState::Finished).await;
        let path = dir.path().join("resolved.bin");
        assert!(path.exists());
        if missing { std::fs::remove_file(&path).unwrap(); }
        sup.remove_with_file(id, delete_file).await.unwrap();
        assert!(sup.queue().get(id).await.is_err());
        assert_eq!(path.exists(), !delete_file && !missing);
        sup.shutdown().await;
    }
}

#[tokio::test]
async fn deleting_a_running_file_waits_for_the_writer_and_uses_the_resolved_name() {
    let mut config = slow(payload(2_000_000));
    config.disposition = Some("attachment; filename=actual.bin".into());
    let server = TestServer::start(config).await;
    let dir = tempfile::tempdir().unwrap();
    let sup = supervisor(Queue::open_in_memory().await.unwrap(), 1);
    let id = sup.enqueue(NewDownload::new(server.url(), dir.path())).await.unwrap();
    wait_for(&sup, id, "bytes", |s| s.downloaded_bytes > 0).await;
    let path = dir.path().join("actual.bin");
    assert!(path.exists());
    sup.remove_with_file(id, true).await.unwrap();
    assert!(!path.exists());
    assert!(!zoltraak_core::resume::ResumeState::sidecar_path(&path).exists());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!path.exists(), "a stopped worker must not recreate the output");
    assert!(sup.queue().get(id).await.is_err());
    sup.shutdown().await;
}

#[tokio::test]
async fn file_deletion_failure_keeps_history_and_never_deletes_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    let queue = Queue::open_in_memory().await.unwrap();
    let id = queue.enqueue(NewDownload::new("https://example.test/blocked", dir.path())).await.unwrap();
    queue.set_state(id, DownloadState::Finished).await.unwrap();
    let path = queue.get(id).await.unwrap().path();
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("keep.txt"), b"keep").unwrap();
    let sup = supervisor(queue.clone(), 1);
    assert!(sup.remove_with_file(id, true).await.is_err());
    assert!(queue.get(id).await.is_ok());
    assert!(path.join("keep.txt").exists());
    sup.remove_with_file(id, false).await.unwrap();
    assert!(path.join("keep.txt").exists());
    sup.shutdown().await;
}

#[tokio::test]
async fn an_unstarted_entry_does_not_delete_an_unrelated_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let queue = Queue::open_in_memory().await.unwrap();
    queue.save_settings(&EngineSettings { schedule: closed_window(), ..EngineSettings::default() }).await.unwrap();
    let id = queue.enqueue(NewDownload::new("https://example.test/untouched.bin", dir.path())).await.unwrap();
    let path = queue.get(id).await.unwrap().path();
    std::fs::write(&path, b"unrelated").unwrap();
    let sup = supervisor(queue.clone(), 1);
    sup.remove_with_file(id, true).await.unwrap();
    assert_eq!(std::fs::read(path).unwrap(), b"unrelated");
    assert!(queue.get(id).await.is_err());
    sup.shutdown().await;
}
