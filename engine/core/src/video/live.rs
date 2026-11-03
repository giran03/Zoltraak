//! Bounded HLS capture. Every complete segment is normalized locally; the
//! durable ledger prevents replaying a rolling window after pause/restart.
use super::{
    net::{base_url, fetch_text},
    stream::{self, ProgressCb, StreamOptions, StreamOutcome, StreamProgress, StreamTick},
};
use crate::{
    error::{Error, Result},
    headers::Headers,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize)]
struct Ledger {
    source: String,
    seconds: u32,
    deadline: i64,
    next: Option<u64>,
    count: usize,
    duration: f64,
    bytes: u64,
    #[serde(default)]
    last_fingerprint: Option<String>,
}
pub(crate) async fn record(
    client: &Client,
    url: &str,
    headers: &Headers,
    dest: &Path,
    cb: Option<ProgressCb<'_>>,
    options: &StreamOptions,
    seconds: u32,
) -> Result<StreamOutcome> {
    if !(5..=1800).contains(&seconds) {
        return Err(Error::InvalidInput(
            "Live recording must be 5–1800 seconds".into(),
        ));
    }
    let dir = dest.with_file_name(format!(
        "{}.live",
        dest.file_name().unwrap_or_default().to_string_lossy()
    ));
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|source| Error::Io {
            path: dir.clone(),
            source,
        })?;
    let journal = dir.join("progress.json");
    let source = super::validation::source_identity(url);
    let mut ledger = match tokio::fs::read(&journal).await {
        Ok(bytes) => {
            let saved: Ledger = serde_json::from_slice(&bytes).map_err(|_| {
                Error::InvalidInput(
                    "Live recording ledger is damaged; retain files and start a new recording"
                        .into(),
                )
            })?;
            if saved.source != source || saved.seconds != seconds || saved.count > 512 {
                return Err(Error::InvalidInput(
                    "Live recording source changed; start a new recording".into(),
                ));
            }
            saved
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ledger {
            source,
            seconds,
            deadline: chrono::Utc::now().timestamp() + seconds as i64,
            next: None,
            count: 0,
            duration: 0.0,
            bytes: 0,
            last_fingerprint: None,
        },
        Err(source) => {
            return Err(Error::Io {
                path: journal.clone(),
                source,
            });
        }
    };
    for i in 0..ledger.count {
        if !tokio::fs::metadata(dir.join(format!("{i}.mp4")))
            .await
            .is_ok_and(|m| m.len() > 0)
        {
            return Err(Error::InvalidInput(
                "A saved live segment is missing; start a new recording".into(),
            ));
        }
    }
    save(&journal, &ledger).await?;
    let paused = |bytes| {
        StreamOutcome::Paused(StreamProgress {
            bytes,
            ..Default::default()
        })
    };
    while ledger.duration < seconds as f64 && chrono::Utc::now().timestamp() < ledger.deadline {
        let text = tokio::select! {
            _ = options.cancel.cancelled() => return Ok(paused(ledger.bytes)),
            result = tokio::time::timeout(std::time::Duration::from_secs(15), fetch_text(client, url, headers)) => result.map_err(|_| Error::InvalidInput("Live playlist reload timed out; captured segments were retained".into()))??,
        };
        let snapshot = super::hls::parse_snapshot(&text, &base_url(url))?;
        if ledger.count == 0 && snapshot.ended {
            return Err(Error::InvalidInput("This HLS playlist has already ended. Use a normal download instead of live recording.".into()));
        }
        if snapshot.entries.is_empty() {
            return Err(Error::InvalidInput(
                "Live media playlist has no complete segments".into(),
            ));
        }
        if ledger.next.is_none() {
            ledger.next = snapshot.entries.last().map(|e| e.sequence);
        }
        let next = ledger.next.unwrap();
        if snapshot
            .entries
            .last()
            .is_some_and(|entry| entry.sequence.saturating_add(1) < next)
        {
            return Err(Error::InvalidInput(
                "Live playlist sequence moved backwards; captured files were retained".into(),
            ));
        }
        if let Some(previous) = snapshot
            .entries
            .iter()
            .find(|entry| entry.sequence.checked_add(1) == Some(next))
        {
            if ledger
                .last_fingerprint
                .as_ref()
                .is_some_and(|saved| *saved != stream::fingerprint(&previous.segments))
            {
                return Err(Error::InvalidInput(
                    "Live segment identity changed; captured files were retained".into(),
                ));
            }
        }
        if snapshot.entries[0].sequence > next {
            return Err(Error::InvalidInput("Live window advanced past a missing segment; captured files were retained instead of splicing a gap".into()));
        }
        let mut added = false;
        for entry in snapshot.entries.iter().filter(|e| e.sequence >= next) {
            if ledger.duration >= seconds as f64
                || chrono::Utc::now().timestamp() >= ledger.deadline
            {
                break;
            }
            if entry.sequence != ledger.next.unwrap() || ledger.count >= 512 {
                return Err(Error::InvalidInput(
                    "Live sequence changed or the 512-segment recording limit was reached".into(),
                ));
            }
            let duration = entry
                .duration
                .ok_or_else(|| Error::InvalidInput("Live segment has no valid duration".into()))?;
            let raw = dir.join(format!("{}.raw", ledger.count));
            let mp4 = dir.join(format!("{}.mp4", ledger.count));
            let mut opts = options.clone();
            opts.resume = StreamProgress::default();
            opts.live_seconds = None;
            let base = ledger.bytes;
            let progress = |tick: StreamTick| {
                if let Some(cb) = cb {
                    cb(StreamTick {
                        bytes: base + tick.bytes,
                        total: None,
                    });
                }
            };
            let outcome = tokio::time::timeout(
                std::time::Duration::from_secs(60),
                stream::download_concat_with_options(
                    client,
                    &entry.segments,
                    headers,
                    &raw,
                    Some(&progress),
                    &opts,
                ),
            )
            .await
            .map_err(|_| {
                Error::InvalidInput(
                    "Live segment download timed out; captured files were retained".into(),
                )
            })??;
            match outcome {
                StreamOutcome::Paused(_) => return Ok(paused(ledger.bytes)),
                StreamOutcome::Refused { status, url, .. } => {
                    return Ok(StreamOutcome::Refused {
                        progress: StreamProgress {
                            bytes: ledger.bytes,
                            ..Default::default()
                        },
                        status,
                        url,
                    });
                }
                StreamOutcome::Completed { bytes } => {
                    tokio::select! { _ = options.cancel.cancelled() => return Ok(paused(ledger.bytes)), result = tokio::time::timeout(std::time::Duration::from_secs(15), super::merge::remux(&raw, &mp4)) => result.map_err(|_| Error::InvalidInput("Live segment normalization timed out; captured files were retained".into()))?? }
                    ledger.bytes += bytes;
                }
            }
            ledger.count += 1;
            ledger.duration += duration;
            ledger.last_fingerprint = Some(stream::fingerprint(&entry.segments));
            ledger.next = entry.sequence.checked_add(1);
            if ledger.next.is_none() {
                return Err(Error::InvalidInput("Live sequence overflow".into()));
            }
            save(&journal, &ledger).await?;
            added = true;
        }
        if snapshot.ended {
            break;
        }
        let delay = if added {
            snapshot.target
        } else {
            snapshot.target / 2.0
        };
        let remaining = (ledger.deadline - chrono::Utc::now().timestamp()).max(0) as f64;
        tokio::select! {
            _ = options.cancel.cancelled() => return Ok(paused(ledger.bytes)),
            _ = tokio::time::sleep(std::time::Duration::from_secs_f64(delay.clamp(0.5, 1800.0).min(remaining))) => {},
        }
    }
    if ledger.count == 0 {
        return Err(Error::InvalidInput(
            "No live segments were captured before the deadline".into(),
        ));
    }
    let inputs = (0..ledger.count)
        .map(|i| dir.join(format!("{i}.mp4")))
        .collect::<Vec<_>>();
    tokio::select! {
        _ = options.cancel.cancelled() => return Ok(paused(ledger.bytes)),
        result = super::merge::join(&inputs, dest) => result?,
    }
    Ok(StreamOutcome::Completed {
        bytes: ledger.bytes,
    })
}
async fn save(path: &Path, ledger: &Ledger) -> Result<()> {
    tokio::fs::write(path, serde_json::to_vec(ledger).unwrap())
        .await
        .map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })
}
pub(crate) async fn cleanup(dest: &Path) {
    let dir = dest.with_file_name(format!(
        "{}.live",
        dest.file_name().unwrap_or_default().to_string_lossy()
    ));
    let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if ["progress.json", "concat.txt"].contains(&name.as_ref())
            || name.split_once('.').is_some_and(|(i, ext)| {
                i.parse::<usize>().is_ok_and(|i| i < 512) && ["raw", "mp4"].contains(&ext)
            })
        {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
    drop(entries);
    let _ = tokio::fs::remove_dir(dir).await;
}

pub(crate) async fn recorded_duration(dest: &Path) -> Option<f64> {
    let dir = dest.with_file_name(format!("{}.live", dest.file_name()?.to_string_lossy()));
    let bytes = tokio::fs::read(dir.join("progress.json")).await.ok()?;
    let ledger: Ledger = serde_json::from_slice(&bytes).ok()?;
    (ledger.duration.is_finite() && ledger.duration > 0.0).then_some(ledger.duration)
}
