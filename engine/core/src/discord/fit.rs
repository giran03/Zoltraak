//! Re-encoding a file so it fits under Discord's upload limit.
//!
//! A download manager mostly produces files far larger than the 10 MiB an
//! unboosted server accepts, so "too large" is the common case, not the edge
//! one. Rather than refuse, Zoltraak makes a *copy* small enough to send. The
//! original on disk is never touched — the point of the download was the file,
//! and the Discord copy is a convenience on top of it.
//!
//! What is possible depends on the file:
//!
//! - **Video and audio** have a duration, so a bitrate that lands inside the
//!   budget can be computed rather than guessed at.
//! - **Images** have no duration; quality and scale are stepped down until one
//!   fits.
//! - **Everything else** — archives, installers, documents — is already
//!   compressed or must stay byte-exact. There is no honest way to shrink a
//!   `.zip` to a quarter of its size, so those are reported as impossible
//!   instead of being mangled.
//!
//! `ffmpeg` and `ffprobe` are expected on `PATH`, the same contract as
//! [`crate::video::merge`].

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::video::merge::missing_ffmpeg;

/// Below this, video stops being worth sending at all — the picture would be a
/// smear. A two-hour film will not fit in 10 MiB, and pretending otherwise
/// wastes minutes of CPU to produce something nobody can watch.
const MIN_VIDEO_BITRATE: u64 = 80_000;
const AUDIO_BITRATE: u64 = 96_000;
const MIN_AUDIO_BITRATE: u64 = 48_000;

/// Heights to encode at, with the bitrate each needs before it is worth using.
/// The ladder is walked downward until the computed bitrate is enough for the
/// height — a low bitrate at 360p looks better than the same bitrate at 1080p.
const LADDER: [(u32, u64); 4] = [(1080, 1_500_000), (720, 800_000), (480, 400_000), (360, 150_000)];

/// How many times to re-encode when the first attempt overshoots the budget.
const ATTEMPTS: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Video,
    Audio,
    Image,
    Other,
}

fn classify(path: &Path) -> Kind {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    match ext.as_str() {
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "flv" | "wmv" | "ts" | "mpg" | "mpeg" | "3gp" | "ogv" => Kind::Video,
        "mp3" | "m4a" | "aac" | "flac" | "wav" | "opus" | "ogg" | "wma" | "alac" => Kind::Audio,
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp" | "tiff" | "tif" | "avif" | "heic" => Kind::Image,
        _ => Kind::Other,
    }
}

/// A directory holding one delivery's re-encoded copy, removed when the
/// delivery is done with it. Tied to the value rather than to a cleanup call
/// so an early return or a failed upload cannot leak it.
#[derive(Debug)]
pub struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new() -> Result<Self> {
        let dir = std::env::temp_dir().join(format!("zoltraak-discord-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).map_err(|source| Error::Io { path: dir.clone(), source })?;
        Ok(Self { dir })
    }

    fn join(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.dir) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(dir = %self.dir.display(), error = %e, "could not remove a Discord scratch directory");
            }
        }
    }
}

pub enum Fitted {
    /// A copy small enough to upload. `scratch` must outlive the upload — it
    /// owns the temporary directory the copy lives in.
    Ready { path: PathBuf, bytes: u64, scratch: Scratch },
    /// Nothing sensible can be done, with the reason to show the user.
    Impossible(String),
}

/// Produce a copy of `path` no larger than `budget` bytes.
///
/// `max_height` caps the ladder; the encoder still steps below it when the
/// bitrate the duration allows would not support that height.
pub async fn fit_to_budget(path: &Path, budget: u64, max_height: u32, cancel: &CancellationToken) -> Result<Fitted> {
    match classify(path) {
        Kind::Other => Ok(Fitted::Impossible(
            "files like this cannot be made smaller without damaging them, so it was left alone".into(),
        )),
        Kind::Image => fit_image(path, budget, cancel).await,
        kind => fit_timed(path, budget, max_height, kind, cancel).await,
    }
}

/// Video and audio: the duration turns a size budget into a bitrate.
async fn fit_timed(path: &Path, budget: u64, max_height: u32, kind: Kind, cancel: &CancellationToken) -> Result<Fitted> {
    let Some(duration) = probe_duration(path).await? else {
        return Ok(Fitted::Impossible("its duration could not be read, so no bitrate could be worked out".into()));
    };
    if duration <= 0.0 {
        return Ok(Fitted::Impossible("it reports no duration, so no bitrate could be worked out".into()));
    }

    let scratch = Scratch::new()?;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let extension = if kind == Kind::Video { "mp4" } else { "m4a" };

    let mut target = budget;
    for attempt in 1..=ATTEMPTS {
        let Some(plan) = plan_bitrate(target, duration, max_height, kind) else {
            return Ok(Fitted::Impossible(format!(
                "it is {:.0} minutes long, which leaves too little bitrate to be worth sending",
                duration / 60.0
            )));
        };
        let out = scratch.join(&format!("{stem}-{attempt}.{extension}"));
        encode(path, &out, &plan, kind, cancel).await?;

        let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(u64::MAX);
        if bytes <= budget {
            tracing::info!(file = %path.display(), bytes, attempt, "re-encoded to fit Discord's limit");
            return Ok(Fitted::Ready { path: out, bytes, scratch });
        }
        tracing::debug!(bytes, budget, attempt, "re-encode overshot the budget; aiming lower");
        // Overshoot means the container and keyframes cost more than the
        // bitrate alone predicted. Aim lower rather than re-deriving the model.
        target = target * 80 / 100;
    }
    Ok(Fitted::Impossible("it could not be squeezed under the limit after three attempts".into()))
}

#[derive(Debug, PartialEq, Eq)]
struct Plan {
    video_bitrate: u64,
    audio_bitrate: u64,
    height: u32,
}

/// Split a size budget into a bitrate the file can be encoded at, or `None`
/// when there is not enough to be worth spending CPU on.
fn plan_bitrate(budget: u64, duration_secs: f64, max_height: u32, kind: Kind) -> Option<Plan> {
    let total_bits = (budget as f64 * 8.0 / duration_secs) as u64;

    if kind == Kind::Audio {
        let audio = total_bits.min(256_000);
        return (audio >= MIN_AUDIO_BITRATE).then_some(Plan { video_bitrate: 0, audio_bitrate: audio, height: 0 });
    }

    // Give audio a smaller share when the whole budget is tight; unintelligible
    // sound makes a clip useless in a way that a soft picture does not.
    let audio = if total_bits > AUDIO_BITRATE * 4 { AUDIO_BITRATE } else { MIN_AUDIO_BITRATE };
    let video = total_bits.saturating_sub(audio);
    if video < MIN_VIDEO_BITRATE {
        return None;
    }
    let height = LADDER
        .iter()
        .filter(|(h, _)| *h <= max_height.max(360))
        .find(|(_, needs)| video >= *needs)
        .map(|(h, _)| *h)
        // Below the whole ladder, but still above the floor: send it at the
        // smallest height rather than refusing.
        .unwrap_or(360);
    Some(Plan { video_bitrate: video, audio_bitrate: audio, height })
}

async fn encode(input: &Path, output: &Path, plan: &Plan, kind: Kind, cancel: &CancellationToken) -> Result<()> {
    let mut command = crate::media_tools::ffmpeg_command();
    command.arg("-y").args(crate::media_tools::LOCAL_INPUT).arg("-i").arg(input);

    if kind == Kind::Video {
        command
            .args(["-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p"])
            // A hard VBV ceiling is what actually keeps the file inside the
            // budget; the average bitrate alone is free to overshoot on busy
            // scenes, which is exactly where a size cap gets broken.
            .args(["-b:v", &format!("{}", plan.video_bitrate)])
            .args(["-maxrate", &format!("{}", plan.video_bitrate)])
            .args(["-bufsize", &format!("{}", plan.video_bitrate * 2)])
            // Never upscale: `min` leaves a small source at its own height.
            .args(["-vf", &format!("scale=-2:min(ih\\,{})", plan.height)])
            .args(["-movflags", "+faststart"])
            .args(["-c:a", "aac", "-b:a", &format!("{}", plan.audio_bitrate), "-ac", "2"]);
    } else {
        command.args(["-vn", "-c:a", "aac", "-b:a", &format!("{}", plan.audio_bitrate)]);
    }
    command.arg(output);

    run(command, "shrink this file to fit Discord's upload limit", cancel).await
}

/// Images have no duration to compute against, so quality and scale are
/// stepped down until something fits. The steps are coarse on purpose: each
/// one is a whole re-encode, and nobody is served by twelve attempts.
async fn fit_image(path: &Path, budget: u64, cancel: &CancellationToken) -> Result<Fitted> {
    let scratch = Scratch::new()?;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("image");

    // WebP holds up far better than JPEG at the small sizes this needs, and
    // Discord renders it inline like any other image.
    for (index, (max_width, quality)) in [(4096u32, 80), (2048, 75), (1280, 65), (800, 55)].into_iter().enumerate() {
        let out = scratch.join(&format!("{stem}-{index}.webp"));
        let mut command = crate::media_tools::ffmpeg_command();
        command
            .arg("-y")
            .args(crate::media_tools::LOCAL_INPUT)
            .arg("-i")
            .arg(path)
            // Only the first frame: an animated GIF or a multi-page TIFF would
            // otherwise become an animation nobody asked for.
            .args(["-frames:v", "1"])
            .args(["-vf", &format!("scale=min(iw\\,{max_width}):-2")])
            .args(["-c:v", "libwebp", "-quality", &quality.to_string()])
            .arg(&out);
        run(command, "shrink this image to fit Discord's upload limit", cancel).await?;

        let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(u64::MAX);
        if bytes <= budget {
            tracing::info!(file = %path.display(), bytes, "re-encoded an image to fit Discord's limit");
            return Ok(Fitted::Ready { path: out, bytes, scratch });
        }
    }
    Ok(Fitted::Impossible("it is still too large even at the lowest quality worth sending".into()))
}

/// How long the file plays, in seconds.
///
/// A file `ffprobe` cannot read is not an error here — some containers simply
/// do not carry a duration — so this returns `None` and lets the caller
/// explain why nothing could be done.
async fn probe_duration(path: &Path) -> Result<Option<f64>> {
    let output = crate::media_tools::ffprobe_command()
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "default=nw=1:nk=1"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(missing_ffmpeg("measure this file before sending it"))?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().parse::<f64>().ok().filter(|d| d.is_finite() && *d > 0.0))
}

/// Run ffmpeg, aborting it if the delivery is cancelled.
///
/// `kill_on_drop` matters more than it looks: without it, cancelling a
/// delivery would leave an encode running at full tilt with nothing waiting
/// for its output.
async fn run(mut command: Command, what: &str, cancel: &CancellationToken) -> Result<()> {
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(missing_ffmpeg(what))?;

    let output = tokio::select! {
        result = child.wait_with_output() => result.map_err(|source| Error::Io { path: PathBuf::from("ffmpeg"), source })?,
        _ = cancel.cancelled() => {
            return Err(Error::InvalidInput("the delivery was cancelled while the file was being re-encoded".into()));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // ffmpeg's last line is the useful one; the rest is a banner.
        let detail = stderr.lines().filter(|l| !l.trim().is_empty()).next_back().unwrap_or("no output").trim();
        return Err(Error::InvalidInput(format!("ffmpeg could not {what}: {detail}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    #[test]
    fn files_are_classified_by_extension() {
        assert_eq!(classify(Path::new("a.MP4")), Kind::Video);
        assert_eq!(classify(Path::new("a.mkv")), Kind::Video);
        assert_eq!(classify(Path::new("a.flac")), Kind::Audio);
        assert_eq!(classify(Path::new("a.png")), Kind::Image);
        assert_eq!(classify(Path::new("a.zip")), Kind::Other);
        assert_eq!(classify(Path::new("a.exe")), Kind::Other);
        assert_eq!(classify(Path::new("noextension")), Kind::Other);
    }

    #[tokio::test]
    async fn an_archive_is_reported_as_impossible_rather_than_mangled() {
        let cancel = CancellationToken::new();
        let fitted = fit_to_budget(Path::new("big.zip"), 10 * MIB, 720, &cancel).await.unwrap();
        match fitted {
            Fitted::Impossible(reason) => assert!(reason.contains("without damaging them"), "{reason}"),
            Fitted::Ready { .. } => panic!("an archive must never be re-encoded"),
        }
    }

    #[test]
    fn a_short_clip_gets_a_generous_bitrate_at_full_height() {
        // 30 seconds into 10 MiB is about 2.8 Mbps.
        let plan = plan_bitrate(budget_of(10 * MIB), 30.0, 1080, Kind::Video).unwrap();
        assert_eq!(plan.height, 1080);
        assert_eq!(plan.audio_bitrate, AUDIO_BITRATE);
        assert!(plan.video_bitrate > 2_000_000, "{plan:?}");
    }

    #[test]
    fn a_longer_clip_steps_down_the_ladder_instead_of_smearing() {
        // 10 minutes into 10 MiB is about 140 kbps: only 360p is honest.
        let plan = plan_bitrate(budget_of(10 * MIB), 600.0, 1080, Kind::Video).unwrap();
        assert_eq!(plan.height, 360);
        assert_eq!(plan.audio_bitrate, MIN_AUDIO_BITRATE, "sound gets the smaller share when space is tight");
    }

    #[test]
    fn the_users_height_cap_is_respected_even_when_the_bitrate_allows_more() {
        let plan = plan_bitrate(budget_of(10 * MIB), 30.0, 480, Kind::Video).unwrap();
        assert_eq!(plan.height, 480);
    }

    #[test]
    fn a_feature_length_film_is_refused_rather_than_wasting_an_hour_of_cpu() {
        assert!(plan_bitrate(budget_of(10 * MIB), 2.0 * 3600.0, 720, Kind::Video).is_none());
    }

    #[test]
    fn a_boosted_server_keeps_the_picture_at_full_height() {
        // 10 minutes into 100 MiB is about 1.4 Mbps — comfortably 720p.
        let plan = plan_bitrate(budget_of(100 * MIB), 600.0, 1080, Kind::Video).unwrap();
        assert!(plan.height >= 720, "{plan:?}");
    }

    #[test]
    fn audio_gets_the_whole_budget_and_a_ceiling() {
        let plan = plan_bitrate(budget_of(10 * MIB), 600.0, 0, Kind::Audio).unwrap();
        assert_eq!(plan.video_bitrate, 0);
        assert!(plan.audio_bitrate >= MIN_AUDIO_BITRATE);
        // A short track must not be encoded at a pointlessly high rate.
        let short = plan_bitrate(budget_of(10 * MIB), 10.0, 0, Kind::Audio).unwrap();
        assert_eq!(short.audio_bitrate, 256_000);
        // An audiobook that cannot fit at a listenable rate is refused.
        assert!(plan_bitrate(budget_of(10 * MIB), 10.0 * 3600.0, 0, Kind::Audio).is_none());
    }

    #[test]
    fn a_scratch_directory_removes_itself() {
        let path = {
            let scratch = Scratch::new().unwrap();
            std::fs::write(scratch.join("x.bin"), b"data").unwrap();
            let path = scratch.dir.clone();
            assert!(path.exists());
            path
        };
        assert!(!path.exists(), "the scratch directory must not outlive the delivery");
    }

    fn budget_of(limit: u64) -> u64 {
        super::super::limits::budget(limit)
    }
}
