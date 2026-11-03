//! Bounded, cached previews of local downloaded media. Network protocols and
//! playlist demuxers are excluded: generating a frame must not fetch a video.
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::Path,
    sync::OnceLock,
    time::Duration,
};
use tokio::sync::Semaphore;

const MAX_JPEG: usize = 1024 * 1024;
const MAX_CACHE_FILES: usize = 64;
static WORKER: OnceLock<Semaphore> = OnceLock::new();

fn jpeg(bytes: &[u8]) -> bool {
    bytes.len() >= 4
        && bytes.len() <= MAX_JPEG
        && bytes.starts_with(&[0xff, 0xd8, 0xff])
        && bytes.ends_with(&[0xff, 0xd9])
}

pub async fn thumbnail(path: &Path, cache: &Path) -> Result<Option<Vec<u8>>, String> {
    let _worker = WORKER
        .get_or_init(|| Semaphore::new(1))
        .acquire()
        .await
        .map_err(|e| e.to_string())?;
    let Ok(metadata) = tokio::fs::metadata(path).await else {
        return Ok(None);
    };
    if !metadata.is_file() || metadata.len() == 0 {
        return Ok(None);
    }
    let mut key = DefaultHasher::new();
    "preview-v1".hash(&mut key);
    tokio::fs::canonicalize(path)
        .await
        .map_err(|e| e.to_string())?
        .hash(&mut key);
    metadata.len().hash(&mut key);
    metadata
        .modified()
        .map_err(|e| e.to_string())?
        .hash(&mut key);
    let cached = cache.join(format!("{:016x}.jpg", key.finish()));
    if let Ok(size) = tokio::fs::metadata(&cached).await {
        if size.len() <= MAX_JPEG as u64 {
            if let Ok(bytes) = tokio::fs::read(&cached).await {
                if jpeg(&bytes) {
                    return Ok(Some(bytes));
                }
            }
        }
    }
    let mut cmd = zoltraak_core::media_tools::ffmpeg_command();
    cmd.kill_on_drop(true).args([
        "-hide_banner", "-loglevel", "error", "-nostdin", "-threads", "1",
        "-protocol_whitelist", "file,pipe",
        "-format_whitelist", "mov,matroska,webm,avi,asf,flv,mpegts,mpeg,mpegvideo,mjpeg,h264,hevc,rm,mxf,nut,yuv4mpegpipe,ogg,mp3,flac,aac,wav,ape,amr,au,image2,jpeg_pipe,png_pipe,webp_pipe,bmp_pipe,tiff_pipe,gif",
        "-probesize", "5000000", "-analyzeduration", "3000000", "-i",
    ]).arg(path).args([
        "-map", "0:v:0", "-frames:v", "1", "-an", "-sn", "-dn",
        "-vf", "scale=w='if(gte(dar,1),640,max(1,trunc(640*dar)))':h='if(gte(dar,1),max(1,trunc(640/dar)),640)',setsar=1",
        "-filter_threads", "1", "-threads", "1", "-q:v", "3", "-f", "image2pipe", "-vcodec", "mjpeg", "pipe:1",
    ]);
    let output = match tokio::time::timeout(Duration::from_secs(12), cmd.output()).await {
        Ok(Ok(output)) if output.status.success() && jpeg(&output.stdout) => output.stdout,
        _ => return Ok(None),
    };
    if tokio::fs::create_dir_all(cache).await.is_ok() {
        let _ = tokio::fs::write(&cached, &output).await;
        prune(cache).await;
    }
    Ok(Some(output))
}

async fn prune(cache: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(cache).await else {
        return;
    };
    let mut files = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.len() != 20
            || !name.ends_with(".jpg")
            || !name[..16].bytes().all(|b| b.is_ascii_hexdigit())
        {
            continue;
        }
        if let Ok(metadata) = entry.metadata().await {
            if metadata.is_file() {
                files.push((metadata.modified().ok(), entry.path()));
            }
        }
    }
    files.sort_by_key(|file| file.0);
    for (_, path) in files
        .iter()
        .take(files.len().saturating_sub(MAX_CACHE_FILES))
    {
        let _ = tokio::fs::remove_file(path).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    fn fixture() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zoltraak-preview-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    async fn clean(dir: &Path) {
        // Remove only known fixture files in our unique temporary directory.
        let mut entries = tokio::fs::read_dir(dir).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            tokio::fs::remove_file(entry.path()).await.unwrap();
        }
        tokio::fs::remove_dir(dir).await.unwrap();
    }
    #[tokio::test]
    async fn completed_video_frame_is_cached_and_changed_files_do_not_reuse_it() {
        let dir = fixture();
        let video = dir.join("clip.mp4");
        tokio::fs::write(
            &video,
            include_bytes!("../../../engine/core/tests/fixtures/verified-av.mp4"),
        )
        .await
        .unwrap();
        let first = thumbnail(&video, &dir)
            .await
            .unwrap()
            .expect("FFmpeg must extract the fixture's frame");
        assert!(jpeg(&first));
        assert_eq!(thumbnail(&video, &dir).await.unwrap().unwrap(), first);
        let cached = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "jpg"))
            .count();
        assert_eq!(cached, 1);
        tokio::fs::write(&video, b"not a media file").await.unwrap();
        assert!(thumbnail(&video, &dir).await.unwrap().is_none());
        assert!(
            thumbnail(&dir.join("missing.mp4"), &dir)
                .await
                .unwrap()
                .is_none()
        );
        clean(&dir).await;
    }
    #[tokio::test]
    async fn portrait_and_anamorphic_frames_keep_their_display_aspect_ratio() {
        let dir = fixture();
        for (name, size, sar, expected) in [
            ("portrait", "180x320", "1", "360x640"),
            ("anamorphic", "320x240", "2", "640x240"),
        ] {
            let video = dir.join(format!("{name}.mp4"));
            let output = zoltraak_core::media_tools::ffmpeg_command()
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("color=c=blue:s={size}:r=1:d=1"),
                    "-vf",
                    &format!("setsar={sar}"),
                    "-c:v",
                    "libx264",
                    "-pix_fmt",
                    "yuv420p",
                    "-y",
                ])
                .arg(&video)
                .output()
                .await
                .unwrap();
            assert!(output.status.success());
            let bytes = thumbnail(&video, &dir).await.unwrap().unwrap();
            let image = dir.join(format!("{name}.jpg"));
            tokio::fs::write(&image, bytes).await.unwrap();
            let probe = zoltraak_core::media_tools::ffprobe_command()
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "stream=width,height",
                    "-of",
                    "csv=s=x:p=0",
                ])
                .arg(&image)
                .output()
                .await
                .unwrap();
            assert_eq!(String::from_utf8(probe.stdout).unwrap().trim(), expected);
        }
        clean(&dir).await;
    }
    #[tokio::test]
    async fn preview_cache_is_bounded_and_preserves_other_files() {
        let dir = fixture();
        for i in 0..70 {
            tokio::fs::write(dir.join(format!("{i:016x}.jpg")), b"cache")
                .await
                .unwrap();
        }
        tokio::fs::write(dir.join("keep.txt"), b"keep")
            .await
            .unwrap();
        prune(&dir).await;
        let count = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "jpg"))
            .count();
        assert_eq!(count, MAX_CACHE_FILES);
        assert_eq!(
            tokio::fs::read(dir.join("keep.txt")).await.unwrap(),
            b"keep"
        );
        clean(&dir).await;
    }
}
