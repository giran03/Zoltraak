//! Continuity boundaries are downloaded separately and joined only locally.
use super::stream::{
    self, ProgressCb, Segment, StreamOptions, StreamOutcome, StreamProgress, StreamTick,
};
use crate::{
    error::{Error, Result},
    headers::Headers,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub(crate) const PREFIX: &str = "zoltraak-stream-groups-v1:";
pub fn encode(groups: &[Vec<Segment>]) -> String {
    format!(
        "{PREFIX}{}",
        serde_json::to_string(groups).expect("segment groups serialize")
    )
}

#[derive(Default, Serialize, Deserialize)]
struct Journal {
    identity: Vec<String>,
    done: usize,
    progress: StreamProgress,
}

pub async fn download(
    client: &Client,
    groups: &[Vec<Segment>],
    headers: &Headers,
    dest: &Path,
    cb: Option<ProgressCb<'_>>,
    options: &StreamOptions,
) -> Result<StreamOutcome> {
    if groups.is_empty()
        || groups.len() > 256
        || groups.iter().any(Vec::is_empty)
        || groups.iter().map(Vec::len).sum::<usize>() > 100_256
    {
        return Err(Error::InvalidInput(
            "Invalid or excessive stream continuity groups".into(),
        ));
    }
    if groups.len() == 1 {
        return stream::download_concat_with_options(
            client, &groups[0], headers, dest, cb, options,
        )
        .await;
    }
    let dir = dest.with_file_name(format!(
        "{}.groups",
        dest.file_name().unwrap_or_default().to_string_lossy()
    ));
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|source| Error::Io {
            path: dir.clone(),
            source,
        })?;
    let journal_path = dir.join("progress.json");
    let identity: Vec<_> = groups.iter().map(|g| stream::fingerprint(g)).collect();
    let mut journal: Journal = tokio::fs::read(&journal_path)
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    if journal.identity != identity || journal.done > groups.len() {
        journal = Journal {
            identity,
            ..Default::default()
        };
    }
    // Completed groups must still exist; never trust a persisted flag alone.
    for i in 0..journal.done {
        if tokio::fs::metadata(dir.join(format!("{i}.mp4")))
            .await
            .map(|m| m.len())
            .unwrap_or(0)
            == 0
        {
            journal.done = i;
            journal.progress = StreamProgress::default();
            break;
        }
    }
    save(&journal_path, &journal).await?;
    let group_identity = format!("groups-v1:{}", journal.identity.join(","));
    let mut bytes = 0;
    for i in 0..journal.done {
        bytes += tokio::fs::metadata(dir.join(format!("{i}.raw")))
            .await
            .map(|m| m.len())
            .unwrap_or(0);
    }
    for (i, segments) in groups.iter().enumerate().skip(journal.done) {
        let raw = dir.join(format!("{i}.raw"));
        let normalized = dir.join(format!("{i}.mp4"));
        let mut opts = options.clone();
        opts.resume = journal.progress.clone();
        let base = bytes;
        let report = |tick: StreamTick| {
            if let Some(cb) = cb {
                cb(StreamTick {
                    bytes: base + tick.bytes,
                    total: None,
                });
            }
        };
        let outcome = stream::download_concat_with_options(
            client,
            segments,
            headers,
            &raw,
            Some(&report),
            &opts,
        )
        .await?;
        let paused = |p: &StreamProgress| StreamProgress {
            bytes: base + p.bytes,
            segments_done: i,
            fingerprint: Some(group_identity.clone()),
        };
        match outcome {
            StreamOutcome::Paused(ref p)
            | StreamOutcome::Refused {
                progress: ref p, ..
            } => {
                journal.progress = p.clone();
                save(&journal_path, &journal).await?;
                return Ok(match outcome {
                    StreamOutcome::Refused { status, url, .. } => StreamOutcome::Refused {
                        progress: paused(p),
                        status,
                        url,
                    },
                    _ => StreamOutcome::Paused(paused(p)),
                });
            }
            StreamOutcome::Completed { bytes: received } => bytes += received,
        }
        tokio::select! {
            _ = options.cancel.cancelled() => return Ok(StreamOutcome::Paused(StreamProgress { bytes, ..Default::default() })),
            result = super::merge::remux(&raw, &normalized) => result?,
        }
        journal.done = i + 1;
        journal.progress = StreamProgress::default();
        save(&journal_path, &journal).await?;
    }
    let inputs: Vec<_> = (0..groups.len())
        .map(|i| dir.join(format!("{i}.mp4")))
        .collect();
    tokio::select! {
        _ = options.cancel.cancelled() => return Ok(StreamOutcome::Paused(StreamProgress { bytes, ..Default::default() })),
        result = super::merge::join(&inputs, dest) => result?,
    }
    // Keep the journal and source groups until final job validation succeeds.
    Ok(StreamOutcome::Completed { bytes })
}

async fn save(path: &Path, journal: &Journal) -> Result<()> {
    let bytes = serde_json::to_vec(journal).map_err(|e| Error::InvalidInput(e.to_string()))?;
    tokio::fs::write(path, bytes)
        .await
        .map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })
}

pub(crate) async fn cleanup(dest: &Path) {
    let dir = dest.with_file_name(format!(
        "{}.groups",
        dest.file_name().unwrap_or_default().to_string_lossy()
    ));
    let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let generated = name.split_once('.').is_some_and(|(index, extension)| {
            index.parse::<usize>().is_ok_and(|i| i < 256) && ["raw", "mp4"].contains(&extension)
        });
        if generated || ["progress.json", "concat.txt"].contains(&name.as_ref()) {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
    drop(entries);
    let _ = tokio::fs::remove_dir(dir).await;
}
