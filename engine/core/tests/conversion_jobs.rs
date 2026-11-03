//! Required conversion recipes against the bundled tools, with probeable outputs.
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use zoltraak_core::{
    Queue,
    conversion::{self, Format, Settings},
    conversion_preview, conversion_watch,
    remux::{
        self, Container,
        manager::{NewRemux, RemuxJob, RemuxJobState, RemuxManager},
    },
};

fn tools() {
    let bundled = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/src-tauri/resources/bin");
    // CARGO_MANIFEST_DIR is engine/core: two parents is the repository.
    zoltraak_core::media_tools::register_search_dir(bundled);
}
fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")).join("../target")).unwrap()
}
fn generate(path: &Path, args: &[&str]) {
    tools();
    let result = zoltraak_core::media_tools::std_ffmpeg_command()
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .arg(path)
        .output()
        .expect("FFmpeg is required for conversion integration tests");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
async fn wait(manager: &RemuxManager, id: zoltraak_core::DownloadId) -> RemuxJob {
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let job = manager.get(id).await.expect("job");
            if job.state.is_terminal() {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("conversion completes within 45 seconds")
}
async fn convert(manager: &RemuxManager, input: &Path, settings: Settings) -> RemuxJob {
    let ids = manager
        .enqueue_conversion(
            NewRemux {
                inputs: vec![input.to_path_buf()],
                container: settings.format.container().unwrap_or(Container::Mp4),
                output_directory: None,
                replace_source: false,
            },
            settings,
        )
        .await
        .expect("valid recipe");
    let job = wait(manager, ids[0]).await;
    assert_eq!(
        job.state,
        RemuxJobState::Finished,
        "{:?} {:?}",
        job.error,
        job.stage
    );
    assert!(input.is_file(), "original preserved");
    job
}
async fn descriptor(job: &RemuxJob) -> remux::MediaInfo {
    remux::probe(job.output.as_ref().expect("output"))
        .await
        .expect("probe")
}
fn video(path: &Path) {
    generate(
        path,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=160x120:rate=12:duration=1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ],
    );
}

#[tokio::test]
async fn video_gif_video_and_same_format_customization() {
    tools();
    let dir = tempdir();
    let input = dir.path().join("clip.mp4");
    video(&input);
    let manager = RemuxManager::new();
    let gif = convert(
        &manager,
        &input,
        Settings {
            format: Format::Gif,
            width: Some(80),
            fps: Some(6.0),
            duration_seconds: Some(0.5),
            ..Default::default()
        },
    )
    .await;
    let info = descriptor(&gif).await;
    assert_eq!(info.width, Some(80));
    assert!(info.audio_codec.is_none());
    let mp4 = convert(
        &manager,
        gif.output.as_ref().unwrap(),
        Settings {
            format: Format::Mp4,
            mode: "encode".into(),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(descriptor(&mp4).await.video_codec.as_deref(), Some("h264"));
    let webp = convert(
        &manager,
        &input,
        Settings {
            format: Format::Webp,
            duration_seconds: Some(0.5),
            ..Default::default()
        },
    )
    .await;
    let webp_info = descriptor(&webp).await;
    if webp_info.unsupported_animation {
        assert!(
            Settings::default()
                .plan(&webp_info)
                .unwrap_err()
                .to_string()
                .contains("cannot decode animated WebP")
        );
    } else {
        let webp_video =
            convert(&manager, webp.output.as_ref().unwrap(), Settings::default()).await;
        assert_eq!(
            descriptor(&webp_video).await.video_codec.as_deref(),
            Some("h264")
        );
    }
    let smaller = convert(
        &manager,
        &input,
        Settings {
            width: Some(100),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(descriptor(&smaller).await.width, Some(100));
    assert_ne!(smaller.output, Some(input));
}

#[tokio::test]
async fn image_roundtrip_and_static_webp() {
    let dir = tempdir();
    let input = dir.path().join("image.png");
    generate(
        &input,
        &[
            "-f",
            "lavfi",
            "-i",
            "color=red:size=81x61,format=rgba",
            "-frames:v",
            "1",
        ],
    );
    let manager = RemuxManager::new();
    let jpg = convert(
        &manager,
        &input,
        Settings {
            format: Format::Jpg,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(descriptor(&jpg).await.width, Some(81));
    let png = convert(
        &manager,
        jpg.output.as_ref().unwrap(),
        Settings {
            format: Format::Png,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(descriptor(&png).await.video_codec.as_deref(), Some("png"));
    let webp = convert(
        &manager,
        &input,
        Settings {
            format: Format::Webp,
            lossless: true,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(descriptor(&webp).await.video_codec.as_deref(), Some("webp"));
}

#[tokio::test]
async fn extracts_audio_and_encodes_all_audio_targets() {
    let dir = tempdir();
    let input = dir.path().join("clip.mp4");
    video(&input);
    let manager = RemuxManager::new();
    let copied = convert(
        &manager,
        &input,
        Settings {
            format: Format::M4a,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(copied.lossless, Some(true));
    assert!(descriptor(&copied).await.video_codec.is_none());
    for format in [
        Format::Mp3,
        Format::M4a,
        Format::Alac,
        Format::Flac,
        Format::Wav,
        Format::Aiff,
        Format::Opus,
        Format::Ogg,
    ] {
        let output = convert(
            &manager,
            &input,
            Settings {
                format,
                mode: "encode".into(),
                channels: Some(1),
                sample_rate: Some(48000),
                normalize: true,
                fade_in: 0.1,
                fade_out: 0.1,
                ..Default::default()
            },
        )
        .await;
        let info = descriptor(&output).await;
        assert!(info.video_codec.is_none());
        assert!(info.audio_codec.is_some());
        assert_eq!(info.audio_tracks[0].channels, Some(1));
    }
}

#[tokio::test]
async fn invalid_batch_is_not_partially_queued_and_retry_keeps_settings() {
    let dir = tempdir();
    let input = dir.path().join("clip.mp4");
    video(&input);
    let manager = RemuxManager::new();
    let result = manager
        .enqueue_conversion(
            NewRemux {
                inputs: vec![input.clone(), dir.path().join("missing.mp4")],
                container: Container::Mp4,
                output_directory: None,
                replace_source: false,
            },
            Settings::default(),
        )
        .await;
    assert!(result.is_err());
    assert!(manager.list().await.is_empty());
    let result = Settings {
        mode: "copy".into(),
        width: Some(80),
        ..Default::default()
    }
    .plan(&remux::probe(&input).await.unwrap());
    assert!(result.is_err());
}

#[tokio::test]
async fn history_recovers_interrupted_jobs_and_clears_terminal_rows() {
    tools();
    let queue = Queue::open_in_memory().await.unwrap();
    let pool = queue.pool();
    let dir = tempdir();
    let input = dir.path().join("clip.mp4");
    video(&input);
    let manager = RemuxManager::with_history(pool.clone()).await.unwrap();
    let job = convert(
        &manager,
        &input,
        Settings {
            format: Format::Mp3,
            ..Default::default()
        },
    )
    .await;
    let mut interrupted = job.clone();
    interrupted.state = RemuxJobState::Running;
    sqlx::query("UPDATE conversion_jobs SET payload=? WHERE id=?")
        .bind(serde_json::to_string(&interrupted).unwrap())
        .bind(job.id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    let recovered = RemuxManager::with_history(pool.clone()).await.unwrap();
    assert_eq!(
        recovered.get(job.id).await.unwrap().state,
        RemuxJobState::Cancelled
    );
    recovered
        .retry(job.id, Some(PathBuf::from("ignored-on-retry")))
        .await
        .unwrap();
    let retried = wait(&recovered, job.id).await;
    assert_eq!(
        retried.state,
        RemuxJobState::Finished,
        "{:?}",
        retried.error
    );
    assert_eq!(
        std::fs::canonicalize(retried.output.as_ref().unwrap().parent().unwrap()).unwrap(),
        std::fs::canonicalize(dir.path()).unwrap()
    );
    recovered.clear_done().await;
    assert!(
        RemuxManager::with_history(pool)
            .await
            .unwrap()
            .list()
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn required_capabilities_are_available() {
    tools();
    let caps = conversion::capabilities().await.unwrap();
    assert!(caps.ffprobe);
    for format in Format::ALL {
        assert!(caps.formats.contains(&format), "missing {format:?}");
    }
}

#[tokio::test]
async fn transparent_images_use_the_selected_background_and_do_not_upscale() {
    let dir = tempdir();
    let input = dir.path().join("transparent.png");
    generate(
        &input,
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=red@0.0:size=80x60,format=rgba",
            "-frames:v",
            "1",
        ],
    );
    let manager = RemuxManager::new();
    let jpg = convert(
        &manager,
        &input,
        Settings {
            format: Format::Jpg,
            background: "#00ff00".into(),
            width: Some(1000),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(descriptor(&jpg).await.width, Some(80));
    let result = zoltraak_core::media_tools::std_ffmpeg_command()
        .args(["-v", "error", "-i"])
        .arg(jpg.output.unwrap())
        .args([
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(
        result.stdout[0] < 20 && result.stdout[1] > 235 && result.stdout[2] < 20,
        "expected green background, got {:?}",
        &result.stdout[..3]
    );
}

#[tokio::test]
async fn simultaneous_outputs_never_clobber_and_source_deletion_is_opt_in() {
    let dir = tempdir();
    let input = dir.path().join("clip.mp4");
    video(&input);
    let existing = dir.path().join("clip.mp3");
    std::fs::write(&existing, b"keep this file").unwrap();
    let manager = RemuxManager::new();
    let ids = manager
        .enqueue_conversion(
            NewRemux {
                inputs: vec![input.clone(), input.clone()],
                container: Container::Mp4,
                output_directory: None,
                replace_source: false,
            },
            Settings {
                format: Format::Mp3,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let a = wait(&manager, ids[0]).await;
    let b = wait(&manager, ids[1]).await;
    assert_eq!(a.state, RemuxJobState::Finished, "{:?}", a.error);
    assert_eq!(b.state, RemuxJobState::Finished, "{:?}", b.error);
    assert_ne!(a.output, b.output);
    assert_eq!(std::fs::read(&existing).unwrap(), b"keep this file");
    assert!(input.is_file());
    let ids = manager
        .enqueue_conversion(
            NewRemux {
                inputs: vec![input.clone()],
                container: Container::Mp4,
                output_directory: None,
                replace_source: true,
            },
            Settings {
                format: Format::Flac,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let job = wait(&manager, ids[0]).await;
    assert_eq!(job.state, RemuxJobState::Finished, "{:?}", job.error);
    assert!(!input.exists());
    assert!(job.output.unwrap().is_file());
}

#[tokio::test]
async fn cancellation_keeps_sources_and_cleans_working_files() {
    let dir = tempdir();
    let input = dir.path().join("image.png");
    generate(
        &input,
        &[
            "-f",
            "lavfi",
            "-i",
            "color=red:size=640x480",
            "-frames:v",
            "1",
        ],
    );
    let manager = RemuxManager::new();
    let ids = manager
        .enqueue_conversion(
            NewRemux {
                inputs: vec![input.clone()],
                container: Container::Mp4,
                output_directory: None,
                replace_source: true,
            },
            Settings {
                duration_seconds: Some(3600.0),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    for _ in 0..100 {
        if manager.get(ids[0]).await.unwrap().state == RemuxJobState::Running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    manager.cancel(ids[0]).await.unwrap();
    assert_eq!(wait(&manager, ids[0]).await.state, RemuxJobState::Cancelled);
    assert!(input.is_file());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn planning_cache_reinspects_changed_sources() {
    let dir = tempdir();
    let input = dir.path().join("image.png");
    generate(
        &input,
        &[
            "-f",
            "lavfi",
            "-i",
            "color=red:size=80x60",
            "-frames:v",
            "1",
        ],
    );
    assert_eq!(
        conversion::inspect_for_plan(&input).await.unwrap().width,
        Some(80)
    );
    assert_eq!(
        conversion::inspect_for_plan(&input).await.unwrap().width,
        Some(80)
    );
    generate(
        &input,
        &[
            "-f",
            "lavfi",
            "-i",
            "color=blue:size=160x120",
            "-frames:v",
            "1",
        ],
    );
    assert_eq!(
        conversion::inspect_for_plan(&input).await.unwrap().width,
        Some(160)
    );
}

#[tokio::test]
async fn conversion_destinations_use_the_working_subfolder_and_allow_overrides() {
    let dir = tempdir();
    let input = dir.path().join("source.mp4");
    video(&input);
    let manager = RemuxManager::new();
    for working in [dir.path().join("working-a"), dir.path().join("working-b")] {
        let output = conversion::default_output_directory(&working);
        let ids = manager
            .enqueue_conversion(
                NewRemux {
                    inputs: vec![input.clone()],
                    container: Container::Mp4,
                    output_directory: Some(output.clone()),
                    replace_source: false,
                },
                Settings {
                    format: Format::Mp3,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let job = wait(&manager, ids[0]).await;
        assert_eq!(job.state, RemuxJobState::Finished, "{:?}", job.error);
        assert_eq!(
            job.output.unwrap(),
            std::path::absolute(output.join("source.mp3")).unwrap()
        );
    }
    let custom = dir.path().join("custom");
    let ids = manager
        .enqueue_conversion(
            NewRemux {
                inputs: vec![input.clone()],
                container: Container::Mp4,
                output_directory: Some(custom.clone()),
                replace_source: false,
            },
            Settings {
                format: Format::Flac,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let job = wait(&manager, ids[0]).await;
    assert_eq!(job.state, RemuxJobState::Finished, "{:?}", job.error);
    assert_eq!(
        job.output.unwrap(),
        std::path::absolute(custom.join("source.flac")).unwrap()
    );
    assert!(input.is_file());
}

#[tokio::test]
async fn filename_template_and_target_size_shape_the_output() {
    tools();
    let dir = tempdir();
    let input = dir.path().join("clip.mp4");
    video(&input);
    let manager = RemuxManager::new();
    let job = convert(
        &manager,
        &input,
        Settings {
            format: Format::Mp4,
            target_size_mb: Some(0.1),
            name_template: "{name}-small-{format}".into(),
            ..Default::default()
        },
    )
    .await;
    let output = job.output.as_ref().expect("output");
    assert_eq!(
        output.file_name().and_then(|n| n.to_str()),
        Some("clip-small-mp4.mp4")
    );
    let bytes = std::fs::metadata(output).unwrap().len() as f64;
    assert!(bytes < 0.1 * 1024.0 * 1024.0 * 1.5, "{bytes} bytes");
    assert_eq!(descriptor(&job).await.video_codec.as_deref(), Some("h264"));
}

#[tokio::test]
async fn crop_rotate_and_flip_reshape_the_picture_and_finished_jobs_describe_the_output() {
    tools();
    let dir = tempdir();
    let input = dir.path().join("clip.mp4");
    video(&input); // 160 x 120
    let manager = RemuxManager::new();
    let job = convert(
        &manager,
        &input,
        Settings {
            format: Format::Mp4,
            crop: Some(conversion::Crop { x: 0.0, y: 0.0, width: 0.5, height: 1.0 }),
            rotate: 90,
            flip_horizontal: true,
            ..Default::default()
        },
    )
    .await;
    let info = descriptor(&job).await;
    // Half the width (80 x 120), then a quarter turn swaps the sides.
    assert_eq!((info.width, info.height), (Some(120), Some(80)));
    let described = job.output_info.as_ref().expect("output described");
    assert_eq!((described.width, described.height), (Some(120), Some(80)));

    let bad = Settings {
        crop: Some(conversion::Crop { x: 0.8, y: 0.0, width: 0.5, height: 1.0 }),
        ..Default::default()
    };
    assert!(bad.validate().is_err());
    let audio = Settings { format: Format::Mp3, rotate: 90, ..Default::default() };
    assert!(audio.validate().is_err());
}

#[tokio::test]
async fn paused_queue_holds_jobs_and_reordering_changes_who_starts_first() {
    tools();
    let dir = tempdir();
    let first = dir.path().join("first.mp4");
    let second = dir.path().join("second.mp4");
    video(&first);
    video(&second);
    let manager = RemuxManager::new();
    manager.set_parallelism(1).unwrap();
    assert!(manager.set_parallelism(0).is_err());
    manager.set_paused(true);
    let mut ids = vec![];
    for input in [&first, &second] {
        let settings = Settings { format: Format::Mp4, mode: "encode".into(), ..Default::default() };
        let request = NewRemux {
            inputs: vec![input.clone()],
            container: Container::Mp4,
            output_directory: Some(dir.path().join("out")),
            replace_source: false,
        };
        ids.extend(manager.enqueue_conversion(request, settings).await.unwrap());
    }
    tokio::time::sleep(Duration::from_millis(700)).await;
    for id in &ids {
        assert_eq!(manager.get(*id).await.unwrap().state, RemuxJobState::Queued, "paused jobs wait");
    }
    assert_eq!(manager.queue_state().queued, ids);
    manager.move_job(ids[1], remux::manager::QueueMove::Top).unwrap();
    assert_eq!(manager.queue_state().queued, vec![ids[1], ids[0]]);
    manager.move_job(ids[1], remux::manager::QueueMove::Down).unwrap();
    manager.move_job(ids[1], remux::manager::QueueMove::Top).unwrap();
    manager.set_paused(false);
    let second_job = wait(&manager, ids[1]).await;
    let first_job = wait(&manager, ids[0]).await;
    assert_eq!(second_job.state, RemuxJobState::Finished);
    assert!(second_job.finished_at <= first_job.finished_at, "the moved job ran first");
    assert!(manager.move_job(ids[0], remux::manager::QueueMove::Top).is_err(), "finished jobs cannot move");
}

#[tokio::test]
async fn previews_render_bounded_samples_and_waveforms_only_while_idle() {
    tools();
    let dir = tempdir();
    let input = dir.path().join("clip.mp4");
    video(&input);
    let frame = conversion_preview::thumbnail(&input, 0.5, 64).await.unwrap();
    assert_eq!(&frame[..2], &[0xFF, 0xD8], "a JPEG");
    let wave = conversion_preview::waveform(&input, 32).await.unwrap();
    assert_eq!(wave.len(), 32);
    assert!(wave.iter().cloned().fold(0.0, f32::max) > 0.1, "the tone is audible");
    assert!(conversion_preview::thumbnail(&dir.path().join("missing.mp4"), 0.0, 64).await.is_err());

    let settings = Settings { format: Format::Mkv, width: Some(80), ..Default::default() };
    let sample = conversion_preview::sample(&input, &settings, 0.2, 99.0).await.unwrap();
    assert_eq!(sample.format, Format::Mp4, "mkv is shown as mp4");
    assert_eq!(sample.width, Some(80));
    assert!(sample.sample_seconds <= conversion_preview::MAX_SAMPLE_SECONDS);
    assert!(!sample.bytes.is_empty());
    let trimmed = Settings { end_seconds: Some(0.8), ..settings.clone() };
    let short = conversion_preview::sample(&input, &trimmed, 0.2, 4.0).await.unwrap();
    assert!((short.sample_seconds - 0.6).abs() < 0.01, "preview honors the selected end");
    let again = conversion_preview::sample(&input, &trimmed, 0.2, 4.0).await.unwrap();
    assert_eq!(again.bytes, short.bytes, "cached sample retains exact output");

    let manager = RemuxManager::new();
    assert!(manager.when_idle(async { Ok(1) }).await.is_ok());
    manager.set_paused(true);
    let request = NewRemux {
        inputs: vec![input.clone()],
        container: Container::Mp4,
        output_directory: Some(dir.path().join("out")),
        replace_source: false,
    };
    manager.enqueue_conversion(request, Settings::default()).await.unwrap();
    assert!(manager.when_idle(async { Ok(1) }).await.is_err(), "waiting work blocks previews");
}

#[tokio::test]
async fn queued_work_preempts_a_preview_even_with_one_slot() {
    tools();
    let dir = tempdir();
    let input = dir.path().join("clip.mp4");
    video(&input);
    let manager = RemuxManager::new();
    manager.set_parallelism(1).unwrap();
    let (started, ready) = tokio::sync::oneshot::channel();
    let preview_manager = manager.clone();
    let preview = tokio::spawn(async move {
        preview_manager.when_idle(async {
            started.send(()).unwrap();
            std::future::pending::<conversion::ConversionPlan>().await;
            Ok(())
        }).await
    });
    ready.await.unwrap();
    let ids = manager.enqueue_conversion(NewRemux { inputs: vec![input], container: Container::Mp4,
        output_directory: Some(dir.path().join("out")), replace_source: false }, Settings::default()).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), preview).await.unwrap().unwrap();
    assert!(result.unwrap_err().to_string().contains("cancelled"));
    assert_eq!(wait(&manager, ids[0]).await.state, RemuxJobState::Finished);
}

#[tokio::test]
async fn measured_loudness_uses_the_selected_range_and_true_peak_limit() {
    tools();
    let dir = tempdir();
    let input = dir.path().join("quiet.wav");
    generate(&input, &["-f", "lavfi", "-i", "sine=frequency=440:duration=3", "-af", "volume=0.1"]);
    let settings = Settings { format: Format::Wav, mode: "encode".into(), normalize: true,
        normalize_two_pass: true, loudness_lufs: -20.0, true_peak_db: -2.0,
        start_seconds: 0.25, end_seconds: Some(2.5), ..Default::default() };
    let manager = RemuxManager::new();
    let job = convert(&manager, &input, settings.clone()).await;
    let output = descriptor(&job).await;
    assert!((output.duration_seconds.unwrap() - 2.25).abs() < 0.1);
    let plan = settings.plan(&remux::probe(&input).await.unwrap()).unwrap();
    let mut measure = settings.clone(); measure.start_seconds = 0.0; measure.end_seconds = None;
    let mut output_plan = plan; output_plan.duration_seconds = output.duration_seconds;
    let result = zoltraak_core::media_tools::std_ffmpeg_command().args(measure.loudness_analysis_args(&output, &output_plan)).output().unwrap();
    assert!(result.status.success());
    let text = String::from_utf8_lossy(&result.stderr);
    let start = text.rfind('{').unwrap();
    let end = start + text[start..].find('}').unwrap();
    let values: serde_json::Value = serde_json::from_str(&text[start..=end]).unwrap();
    let lufs: f64 = values["input_i"].as_str().unwrap().parse().unwrap();
    let peak: f64 = values["input_tp"].as_str().unwrap().parse().unwrap();
    assert!((lufs + 20.0).abs() < 1.0, "measured {lufs} LUFS");
    assert!(peak <= -1.9, "true peak {peak}");
    let sample = conversion_preview::sample(&input, &settings, 0.25, 1.0).await.unwrap();
    assert!(!sample.bytes.is_empty());
}

#[tokio::test]
async fn watch_rules_convert_only_files_that_arrive_later() {
    tools();
    let dir = tempdir();
    let watched = dir.path().join("watched");
    std::fs::create_dir_all(&watched).unwrap();
    let old = watched.join("old.mp4");
    video(&old);
    let watcher = conversion_watch::ConversionWatcher::in_memory();
    watcher
        .set_rules(vec![conversion_watch::WatchRule {
            id: "mp4s".into(),
            folder: watched.clone(),
            extensions: vec!["mp4".into()],
            settings: Settings { format: Format::Webm, mode: "encode".into(), ..Default::default() },
            output_directory: None,
            enabled: true,
        }])
        .unwrap();
    let manager = RemuxManager::new();
    watcher.poll(&manager).await; // sets the existing file aside
    let fresh = watched.join("fresh.mp4");
    video(&fresh);
    watcher.poll(&manager).await; // first sighting
    watcher.poll(&manager).await; // unchanged, so it is converted
    let jobs = manager.list().await;
    assert_eq!(jobs.len(), 1, "only the new file");
    let job = wait(&manager, jobs[0].id).await;
    assert_eq!(job.state, RemuxJobState::Finished, "{:?}", job.error);
    let output = job.output.unwrap();
    assert!(output.canonicalize().unwrap().starts_with(watched.canonicalize().unwrap().join("Converted")));
    assert_eq!(output.extension().and_then(|e| e.to_str()), Some("webm"));
    assert_eq!(watcher.statuses()[0].queued, 1);
}
